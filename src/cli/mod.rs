//! CLI surface: command-tree definition and dispatch.
//!
//! A `clap` derive tree owns flag parsing and the generated help.
//!
//! The command tree lives in [`args`]; the per-command handlers live in
//! [`commands`], grouped by concern. This module is the thin coordinator: parse,
//! dispatch, and the shared context/iteration helpers the handlers reuse.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};
use clap::Parser;

use crate::adapters::{config_dir_from_env, resolve_subagents_dir_for_session};
use crate::core::{DetectInput, DispatchMechanism, RunContext, detect_run_context};

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
            run_mode: None,
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
        Commands::SwitchCondition(args) => run_switch_condition(args),
        Commands::ResetBatch(args) => run_reset_batch(args),
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
        run_mode: args.run_mode,
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
/// Always names `--skill-dir`, `--skill`, and `--workspace-dir` (all three are
/// always populated in [`RunContext`] and always re-resolve), so the printed
/// "Next:" commands are copy-pasteable from any cwd — not just the one `run`
/// happened to start in. The absolute `--workspace-dir` is what lets the isolated
/// session run `ingest`/`finalize`/`switch-condition` from `cwd = iteration-N/env/`:
/// without it, `workspace_root` would default to `<cwd>/.eval-magic`
/// (`detect_run_context`) and the iteration tree above the env would not resolve.
pub(crate) fn command_target_args(ctx: &RunContext) -> String {
    format!(
        " --skill-dir {} --skill {} --workspace-dir {} --run-mode {}",
        ctx.skill_dir.display(),
        ctx.skill_name,
        ctx.workspace_root.display(),
        ctx.run_mode.as_str()
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

/// The env directories a run staged under `iteration_dir`: the single `env/` for
/// the InSession mechanism, or one `env-<group>-<condition>/` per `(group, condition)`
/// for Cli. A best-effort directory scan (returns empty when the dir can't be read),
/// used by `teardown`/`finalize` to walk every env's write guard. Preferred over
/// reading `dispatch.json` because it has no parse-failure mode, needs no path
/// re-basing (recorded env dirs can be relative), and the only `env`/`env-*` children
/// of an iteration dir are the staged envs.
pub(crate) fn staged_env_roots(iteration_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(iteration_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name == "env" || name.starts_with("env-")
        })
        .map(|e| e.path())
        .collect()
}

/// Resolve the subagents transcript dir for an in-session stage that reads
/// transcripts. The subagents dir is the `InSession` transcript source, so this
/// is keyed on the dispatch *mechanism*, not the harness: `Cli`-mechanism runs
/// (Codex; Claude Code hybrid/headless) read each task's `outputs/<events>.jsonl`
/// and resolve to `None` — they must never bail on a missing
/// `CLAUDE_CODE_SESSION_ID`. For the `InSession` mechanism (Claude Code
/// interactive), precedence is: an explicit `--subagents-dir` (validated to
/// exist) wins; otherwise resolve from a session id — the `--session-id` flag if
/// given, else the `CLAUDE_CODE_SESSION_ID` env var Claude Code sets in the
/// orchestrating agent's shell — locating
/// `<config>/projects/<cwd-slug>/<session-id>/subagents/` (scanning `projects/*`
/// if the cwd slug differs).
pub(crate) fn resolve_subagents_dir(
    mechanism: DispatchMechanism,
    subagents_dir: Option<&str>,
    session_id: Option<&str>,
) -> anyhow::Result<Option<PathBuf>> {
    if mechanism != DispatchMechanism::InSession {
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

    /// The isolated session runs `ingest`/`finalize`/`switch-condition` from
    /// `cwd = iteration-N/env/`. Without an explicit workspace root those commands
    /// default `workspace_root` to `<cwd>/.eval-magic` and bail "not found",
    /// so the selector must carry an absolute `--workspace-dir` pointing at the
    /// real workspace above the env.
    #[test]
    fn target_args_carry_absolute_workspace_dir() {
        let tmp = TempDir::new().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let skill_subdir = make_skill(&root, "skills", "mr-review");

        let ctx = detect_run_context(DetectInput {
            cwd: Some(skill_subdir),
            ..Default::default()
        })
        .unwrap();

        let args = command_target_args(&ctx);
        assert!(
            args.contains(&format!("--workspace-dir {}", ctx.workspace_root.display())),
            "selector names absolute --workspace-dir: {args}"
        );
        assert!(
            ctx.workspace_root.is_absolute(),
            "workspace_root is absolute: {}",
            ctx.workspace_root.display()
        );

        // Round-trip from an env-like cwd below the workspace: feeding the
        // selector's roots back resolves the SAME workspace, not
        // `<cwd>/.eval-magic`.
        let env_like = ctx
            .workspace_root
            .join("mr-review")
            .join("iteration-1")
            .join("env");
        fs::create_dir_all(&env_like).unwrap();
        let resolved = detect_run_context(DetectInput {
            skill_dir: Some(ctx.skill_dir.display().to_string()),
            skill: Some(ctx.skill_name.clone()),
            workspace_dir: Some(ctx.workspace_root.display().to_string()),
            cwd: Some(env_like),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(resolved.workspace_root, ctx.workspace_root);
    }

    #[test]
    fn resolve_subagents_dir_is_none_for_cli_mechanism() {
        // The subagents dir is the InSession transcript source. Cli-mechanism
        // runs (Codex; Claude Code hybrid/headless) read each task's events file,
        // so resolution is a no-op — and must NOT bail on a missing
        // CLAUDE_CODE_SESSION_ID. This is the regression: the old harness-keyed
        // gate forced session resolution for Claude Code and aborted under
        // hybrid/headless. The Cli arm returns before reading any env var, so this
        // is deterministic regardless of the test runner's environment.
        assert_eq!(
            resolve_subagents_dir(DispatchMechanism::Cli, None, None).unwrap(),
            None
        );
        // A passed --subagents-dir is ignored in Cli mode (the events file is the
        // source), so it resolves to None without touching the filesystem.
        assert_eq!(
            resolve_subagents_dir(DispatchMechanism::Cli, Some("/whatever"), None).unwrap(),
            None
        );
    }

    #[test]
    fn resolve_subagents_dir_uses_existing_explicit_dir() {
        // InSession (Claude Code interactive): an explicit, existing
        // --subagents-dir wins over any session-id resolution.
        let tmp = TempDir::new().unwrap();
        let resolved = resolve_subagents_dir(
            DispatchMechanism::InSession,
            Some(&tmp.path().display().to_string()),
            None,
        )
        .unwrap();
        assert_eq!(resolved, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn resolve_subagents_dir_errors_when_explicit_dir_missing() {
        // InSession with an explicit --subagents-dir that doesn't exist is a hard
        // error (not a silent fallback to session-id resolution).
        let err = resolve_subagents_dir(
            DispatchMechanism::InSession,
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
