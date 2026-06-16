//! CLI surface: command-tree definition and dispatch.
//!
//! A `clap` derive tree owns flag parsing and the generated help.
//!
//! The command tree lives in [`args`]; the per-command handlers live in
//! [`commands`], grouped by concern. This module is the thin coordinator: parse,
//! dispatch, and the shared context/iteration helpers the handlers reuse.

use std::path::PathBuf;

use anyhow::{anyhow, bail};
use clap::Parser;

use crate::adapters::{config_dir_from_env, resolve_subagents_dir_for_session};
use crate::core::{DetectInput, Harness, RunContext, detect_run_context};

mod args;
mod commands;
mod help;
mod run;

use args::{Cli, Commands, CommonArgs, RunArgs};
use commands::*;

/// Parse process arguments, dispatch to the selected subcommand, and return its
/// result. Called by the binary entry point.
pub fn run() -> anyhow::Result<()> {
    dispatch(Cli::parse().command)
}

fn dispatch(command: Option<Commands>) -> anyhow::Result<()> {
    // No subcommand means the default `run` action.
    let command = command.unwrap_or(Commands::Run(RunArgs {
        common: CommonArgs {
            skill_dir: None,
            skill: None,
            iteration: None,
            mode: None,
            harness: None,
            workspace_dir: None,
            subagents_dir: None,
            session_id: None,
            only: None,
            skip: None,
            overwrite: false,
        },
        baseline: None,
        bootstrap: None,
        dry_run: false,
        no_stage: false,
        guard: false,
        stage_name: None,
        plan_mode: false,
        runs: 1,
        agent_model: None,
        judge_model: None,
        label: None,
    }));

    match command {
        Commands::Run(args) => run_run(args),
        Commands::Ingest(args) => run_ingest(args),
        Commands::Finalize(args) => run_finalize(args),
        Commands::Init(args) => run_init(args),
        Commands::Validate(args) => run_validate(args),
        Commands::TeardownGuard(_) => run_teardown_guard(),
        Commands::Guard { marker } => run_guard(marker),
        Commands::GuardCodex { marker } => run_guard_codex(marker),
        Commands::RecordRuns(args) => run_record_runs(args),
        Commands::FillTranscripts(args) => run_fill_transcripts(args),
        Commands::DetectStrayWrites(args) => run_detect_stray_writes(args),
        Commands::Grade(args) => run_grade(args),
        Commands::Aggregate(args) => run_aggregate(args),
        Commands::Snapshot(args) => run_snapshot(args),
        Commands::Teardown(args) => run_teardown(args),
        Commands::PromoteBaseline(args) => run_promote_baseline(args),
    }
}

/// Resolve a [`RunContext`] from the shared flags (skill dir/name, workspace,
/// harness). Used by every post-dispatch stage handler.
pub(crate) fn run_context_from(args: &CommonArgs) -> anyhow::Result<RunContext> {
    run_context_with_bootstrap(args, None)
}

/// Like [`run_context_from`], but threads an optional `--bootstrap` file (only
/// the `run` orchestrator consumes it; post-dispatch stages pass `None`).
pub(crate) fn run_context_with_bootstrap(
    args: &CommonArgs,
    bootstrap: Option<String>,
) -> anyhow::Result<RunContext> {
    Ok(detect_run_context(DetectInput {
        skill_dir: args.skill_dir.clone(),
        skill: args.skill.clone(),
        bootstrap,
        workspace_dir: args.workspace_dir.clone(),
        harness: args.harness,
        cwd: None,
    })?)
}

/// Split a comma-separated `--only`/`--skip` value into trimmed, non-empty ids.
pub(crate) fn parse_id_list(v: Option<&str>) -> Option<Vec<String>> {
    v.map(|s| {
        s.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    })
}

/// Render a fully self-sufficient target selector for the current run context.
///
/// Always names both `--skill-dir` and `--skill` (both are always populated in
/// [`RunContext`] and always re-resolve), so the printed "Next:" commands are
/// copy-pasteable from any cwd — not just the one `run` happened to start in.
pub(crate) fn command_target_args(ctx: &RunContext) -> String {
    format!(
        " --skill-dir {} --skill {}",
        ctx.skill_dir.display(),
        ctx.skill_name
    )
}

/// Resolve the explicit iteration, or default to the latest existing
/// `iteration-<n>` under `<workspace>/<skill>`.
pub(crate) fn resolve_iteration(ctx: &RunContext, iteration: Option<u32>) -> anyhow::Result<u32> {
    if let Some(iteration) = iteration {
        return Ok(iteration);
    }

    let skill_workspace = ctx.workspace_root.join(&ctx.skill_name);
    let entries = std::fs::read_dir(&skill_workspace).map_err(|_| {
        anyhow!(
            "missing --iteration (no iterations found for {})",
            ctx.skill_name
        )
    })?;
    entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_prefix("iteration-")
                .and_then(|n| n.parse::<u32>().ok())
        })
        .max()
        .ok_or_else(|| {
            anyhow!(
                "missing --iteration (no iterations found for {})",
                ctx.skill_name
            )
        })
}

/// The iteration directory for a run: `<workspace>/<skill>/iteration-<n>`.
/// Defaults to the latest existing iteration when `--iteration` is absent.
pub(crate) fn iteration_dir(ctx: &RunContext, iteration: Option<u32>) -> anyhow::Result<PathBuf> {
    let iteration = resolve_iteration(ctx, iteration)?;
    let dir = ctx
        .workspace_root
        .join(&ctx.skill_name)
        .join(format!("iteration-{iteration}"));
    if !dir.is_dir() {
        bail!("not found: {}", dir.display());
    }
    Ok(dir)
}

/// Resolve the subagents transcript dir for a Claude Code stage that reads
/// transcripts. Precedence: an explicit `--subagents-dir` (validated to exist)
/// wins; otherwise resolve from a session id — the `--session-id` flag if given,
/// else the `CLAUDE_CODE_SESSION_ID` env var Claude Code sets in the
/// orchestrating agent's shell — locating
/// `<config>/projects/<cwd-slug>/<session-id>/subagents/` (scanning `projects/*`
/// if the cwd slug differs). Codex/OpenCode read `outputs/codex-events.jsonl`,
/// so they resolve to `None`.
pub(crate) fn resolve_subagents_dir(
    harness: Harness,
    subagents_dir: Option<&str>,
    session_id: Option<&str>,
) -> anyhow::Result<Option<PathBuf>> {
    if harness != Harness::ClaudeCode {
        return Ok(None);
    }
    if let Some(dir) = subagents_dir {
        let path = PathBuf::from(dir);
        if !path.exists() {
            bail!("subagents-dir not found: {}", path.display());
        }
        return Ok(Some(path));
    }
    let session = session_id
        .map(str::to_string)
        .or_else(|| std::env::var("CLAUDE_CODE_SESSION_ID").ok())
        .filter(|s| !s.trim().is_empty());
    let Some(session) = session else {
        bail!(
            "could not auto-resolve the subagents dir: CLAUDE_CODE_SESSION_ID is not set. \
             Re-run inside the Claude Code session that dispatched the subagents, or pass \
             --session-id <id> or --subagents-dir <path>."
        );
    };
    let config_dir = config_dir_from_env();
    let cwd = std::env::current_dir()?;
    match resolve_subagents_dir_for_session(&config_dir, &cwd, &session) {
        Some(path) => Ok(Some(path)),
        None => bail!(
            "no subagents dir found for session {session} under {}/projects/. The session may \
             not have dispatched any subagents (or lives under a different CLAUDE_CONFIG_DIR). \
             Pass --subagents-dir <path> to override.",
            config_dir.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    /// Create `<root>/<parent>/<name>/SKILL.md` and return the skill subdir.
    fn make_skill(root: &Path, parent: &str, name: &str) -> PathBuf {
        let subdir = root.join(parent).join(name);
        fs::create_dir_all(&subdir).unwrap();
        fs::write(
            subdir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test\n---\n\nbody\n"),
        )
        .unwrap();
        subdir
    }

    /// The selector must be copy-pasteable: even when `run` was invoked from
    /// inside the skill dir (the case that used to render an empty selector), it
    /// must name both `--skill-dir` and `--skill`, and re-resolve to the same
    /// skill from an unrelated cwd.
    #[test]
    fn target_args_are_self_sufficient_when_run_from_inside_skill_dir() {
        let tmp = TempDir::new().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let skill_subdir = make_skill(&root, "skills", "mr-review");

        // Mimic `run` started from inside the skill dir: no --skill-dir/--skill.
        let ctx = detect_run_context(DetectInput {
            cwd: Some(skill_subdir.clone()),
            ..Default::default()
        })
        .unwrap();

        let args = command_target_args(&ctx);
        assert!(
            args.contains("--skill-dir"),
            "selector names --skill-dir: {args}"
        );
        assert!(
            args.contains("--skill mr-review"),
            "selector names --skill: {args}"
        );

        // Round-trip: feeding the rendered selector back from an unrelated cwd
        // resolves the same skill.
        let other = root.join("elsewhere");
        fs::create_dir_all(&other).unwrap();
        let resolved = detect_run_context(DetectInput {
            skill_dir: Some(ctx.skill_dir.display().to_string()),
            skill: Some(ctx.skill_name.clone()),
            cwd: Some(other),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(resolved.skill_subdir, ctx.skill_subdir);
    }

    #[test]
    fn resolve_subagents_dir_is_none_for_non_claude_harness() {
        // Codex/OpenCode never read a subagents dir, so resolution is a no-op
        // even when a dir is passed.
        assert_eq!(
            resolve_subagents_dir(Harness::Codex, Some("/whatever"), None).unwrap(),
            None
        );
        assert_eq!(
            resolve_subagents_dir(Harness::OpenCode, None, None).unwrap(),
            None
        );
    }

    #[test]
    fn resolve_subagents_dir_uses_existing_explicit_dir() {
        let tmp = TempDir::new().unwrap();
        let resolved = resolve_subagents_dir(
            Harness::ClaudeCode,
            Some(&tmp.path().display().to_string()),
            None,
        )
        .unwrap();
        assert_eq!(resolved, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn resolve_subagents_dir_errors_when_explicit_dir_missing() {
        let err = resolve_subagents_dir(
            Harness::ClaudeCode,
            Some("/no/such/subagents/dir/xyz"),
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("subagents-dir not found"),
            "got: {err}"
        );
    }
}
