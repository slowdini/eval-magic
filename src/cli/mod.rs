//! CLI surface: the command tree, its handlers, and the `run` orchestrator.
//!
//! A `clap` derive tree owns flag parsing and the generated help.
//!
//! - [`args`] — the command tree and shared flag documentation (the primary
//!   documentation surface); command-specific argument modules keep focused
//!   additions out of that large tree, and [`help`] holds worked examples.
//! - [`commands`] — one thin handler per subcommand, grouped by concern. Each
//!   maps parsed args onto a library module and renders the result.
//! - [`run`] — the `run` orchestrator. This is the bulk of the module: staging,
//!   dispatch plan assembly, and the `ingest`/`finalize` chains. It lives here
//!   rather than in a library module because it is a CLI-shaped workflow —
//!   it drives the operator hand-off, not just data transformation — and it
//!   carries its own unit tests (`run/staging/tests/`, `run/dispatch/tests/`,
//!   `run/golden_tests.rs`).
//!
//! This file itself is the coordinator: parse, dispatch, and the shared
//! context/iteration helpers the handlers reuse.
//!
//! User-facing output is this module's job. Library modules (`pipeline`,
//! `workspace`, `sandbox`, `adapters`) return warnings on their result structs;
//! the handlers here print them, prefixed `⚠ ` — so there is exactly one place
//! that decides how a warning reads.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};
use clap::Parser;

use crate::core::fs::artifact_path;
use crate::core::{DetectInput, Harness, RunContext, detect_run_context};

mod args;
mod commands;
mod compare_args;
mod help;
mod init_args;
mod run;

use args::{Cli, Commands, CommonArgs, RunArgs};
use commands::*;

/// Parse process arguments, initialize the harness descriptor registry (every
/// layer, including an optional `--harness-file`), dispatch to the selected
/// subcommand, and return its result. Called by the binary entry point.
pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let lint_as_builtin = match &cli.command {
        Some(Commands::Harness(args)) => matches!(
            &args.command,
            args::HarnessCommands::Lint {
                as_builtin: true,
                ..
            }
        ),
        _ => false,
    };
    if lint_as_builtin && cli.harness_file.is_some() {
        anyhow::bail!(
            "--as-builtin cannot be used with --harness-file; pass the descriptor as the lint \
             target instead"
        );
    }
    // The hidden guard hooks fire on every PreToolUse in a dispatched session
    // and only ever need the embedded descriptors — skip layered discovery so
    // a broken user descriptor can't add noise or latency per tool call (the
    // lazy registry fallback serves them embedded-only).
    let is_guard_hook = matches!(
        cli.command,
        Some(Commands::Guard { .. } | Commands::GuardCodex { .. } | Commands::GuardHook { .. })
    );
    if !is_guard_hook {
        crate::adapters::registry::init_registry(cli.harness_file.as_deref().map(Path::new))?;
    }
    dispatch(cli.command, cli.harness_file.as_deref())
}

fn dispatch(command: Option<Commands>, harness_file: Option<&str>) -> anyhow::Result<()> {
    // No subcommand means the default `run` action.
    let command = command.unwrap_or(Commands::Run(RunArgs {
        common: CommonArgs {
            skill_dir: None,
            skill: None,
            iteration: None,
            mode: None,
            harness: None,
            workspace_dir: None,
            only: None,
            skip: None,
            overwrite: false,
        },
        baseline: None,
        bootstrap: None,
        dry_run: false,
        no_stage: false,
        guard: false,
        no_guard: false,
        stage_name: None,
        plan_mode: false,
        runs: 1,
        agent_model: None,
        agent_env: Vec::new(),
        judge_model: None,
        judge_samples: 1,
        responder_model: None,
        label: None,
    }));

    match command {
        Commands::Run(args) => run_run(args),
        Commands::Dispatch(args) => run_dispatch(args),
        Commands::Ingest(args) => run_ingest(args),
        Commands::Compare(args) => run_compare(args),
        Commands::Finalize(args) => run_finalize(args),
        Commands::Init(args) => run_init(args),
        Commands::Validate(args) => run_validate(args),
        Commands::TeardownGuard(args) => run_teardown_guard(args),
        Commands::Guard { marker } => run_guard(marker),
        Commands::GuardCodex { marker } => run_guard_codex(marker),
        Commands::GuardHook { harness, marker } => run_guard_hook(&harness, marker),
        Commands::Fixture(args) => run_fixture(args),
        Commands::RecordRuns(args) => run_record_runs(args),
        Commands::DetectStrayWrites(args) => run_detect_stray_writes(args),
        Commands::Grade(args) => run_grade(args),
        Commands::Aggregate(args) => run_aggregate(args),
        Commands::Harness(args) => run_harness(args, harness_file),
        Commands::Docs { topic } => run_docs(topic),
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
    // `--harness` parses as a plain string so runtime-loaded descriptors can
    // name harnesses clap never saw; resolution against the registry happens
    // here, after parsing.
    let harness = args.harness.as_deref().map(Harness::resolve).transpose()?;
    let ctx = detect_run_context(DetectInput {
        skill_dir: args.skill_dir.clone(),
        skill: args.skill.clone(),
        bootstrap,
        workspace_dir: args.workspace_dir.clone(),
        harness,
        harness_file: crate::adapters::registry::session_harness_file().map(Path::to_path_buf),
        cwd: None,
    })?;
    // `core` returns warnings rather than printing them; this is the one place
    // every run-loop command passes through, so it is where they are shown.
    for warning in &ctx.warnings {
        eprintln!("⚠ {warning}");
    }
    Ok(ctx)
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
/// The selector reproduces the invocation that built the context, so a printed
/// "Next:" command is copy-pasteable from any cwd AND re-runs the same
/// experiment (#294):
///
/// * `--skill-dir … --skill …` only when the invocation used `--skill-dir`
///   (`stage_siblings`); otherwise `--skill` names the absolute skill subdir.
///   Inventing a `--skill-dir` would stage every sibling skill ambiently — a
///   different experiment from the one just prepared.
/// * an absolute `--workspace-dir`, so the human can run `ingest`/`finalize`
///   from a per-`(group, condition)` env dir: without it, `workspace_root`
///   would fall back to the derived default (`detect_run_context`), which is
///   keyed on the skill directory rather than on the cwd, and the iteration
///   tree above the env would not resolve.
/// * `--harness-file …` when the invocation loaded one; dropping it silently
///   resolves a different descriptor than the run was prepared with.
pub(crate) fn command_target_args(ctx: &RunContext) -> String {
    let mut args = String::new();
    if ctx.stage_siblings {
        args.push_str(&format!(
            " --skill-dir {} --skill {}",
            artifact_path(&ctx.skill_dir),
            ctx.skill_name
        ));
    } else {
        args.push_str(&format!(" --skill {}", artifact_path(&ctx.skill_subdir)));
    }
    args.push_str(&format!(
        " --workspace-dir {}",
        artifact_path(&ctx.workspace_root)
    ));
    if let Some(file) = &ctx.harness_file {
        args.push_str(&format!(" --harness-file {}", artifact_path(file)));
    }
    args
}

/// The warn-loudly backstop for #294. Post-prep stages resolve the harness
/// descriptor by label, so a follow-up that drops `--harness-file` (or runs
/// where project descriptor layers differ) silently switches descriptors
/// mid-campaign while the iteration's artifacts keep the prep-time
/// declarations. Returns the warning when this invocation's resolved
/// descriptor differs from the one the iteration was prepared with; `None`
/// when they match, or when the iteration predates descriptor provenance.
pub(crate) fn harness_descriptor_drift_warning(
    ctx: &RunContext,
    iteration_dir: &Path,
) -> Option<String> {
    let raw = std::fs::read_to_string(iteration_dir.join("conditions.json")).ok()?;
    let conditions: crate::core::ConditionsRecord = serde_json::from_str(&raw).ok()?;
    let prepared = conditions.harness_descriptor_digest?;
    let current = crate::adapters::registry::descriptor_digest(ctx.harness);
    if prepared == current {
        return None;
    }
    let label = ctx.harness.name();
    Some(match (&conditions.harness_file, &ctx.harness_file) {
        (Some(file), None) => format!(
            "harness descriptor drift: this iteration was prepared with --harness-file {file} (descriptor digest {prepared}), but this invocation resolved '{label}' as digest {current}. Re-run with --harness-file {file}: the iteration's dispatch templates and shadow declarations came from that descriptor."
        ),
        _ => format!(
            "harness descriptor drift: '{label}' resolves to digest {current}, but this iteration was prepared with digest {prepared} — descriptor files changed since prep. Stages resolve the descriptor by label, so continuing mixes two descriptors in one comparison."
        ),
    })
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

/// The env directories a run staged under `iteration_dir`: one
/// `env-<group>-<condition>/` per `(group, condition)`. A best-effort directory
/// scan (returns empty when the dir can't be read), used by `teardown`/`finalize`
/// to walk every env's write guard. Preferred over reading `dispatch.json` because
/// it has no parse-failure mode, needs no path re-basing (recorded env dirs can be
/// relative), and the only `env-*` children of an iteration dir are the staged envs.
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

    /// The selector must be copy-pasteable *and* behavior-preserving (#294):
    /// even when `run` was invoked from inside the skill dir (the case that
    /// used to render an empty selector), it names `--skill` as an absolute
    /// path and re-resolves to the same skill from an unrelated cwd. It must
    /// NOT invent a `--skill-dir` the invocation never used — `--skill-dir`
    /// sets `stage_siblings`, so re-running from that selector would stage
    /// every sibling skill ambiently and run a different experiment.
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
            !args.contains("--skill-dir"),
            "an invocation without --skill-dir must not gain one: {args}"
        );
        assert!(
            args.contains(&format!("--skill {}", artifact_path(&skill_subdir))),
            "selector names --skill as an absolute path: {args}"
        );

        // Round-trip: feeding the rendered selector back from an unrelated cwd
        // resolves the same skill with the same staging behavior.
        let other = root.join("elsewhere");
        fs::create_dir_all(&other).unwrap();
        let resolved = detect_run_context(DetectInput {
            skill: Some(ctx.skill_subdir.display().to_string()),
            cwd: Some(other),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(resolved.skill_subdir, ctx.skill_subdir);
        assert!(!resolved.stage_siblings);
    }

    /// `--skill-dir <dir> --skill <name>` is the sibling-staging form, so when
    /// the invocation used it the selector reproduces it exactly.
    #[test]
    fn target_args_keep_skill_dir_when_the_invocation_used_it() {
        let tmp = TempDir::new().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let skill_subdir = make_skill(&root, "skills", "mr-review");
        let skill_dir = skill_subdir.parent().unwrap().to_path_buf();

        let ctx = detect_run_context(DetectInput {
            skill_dir: Some(skill_dir.display().to_string()),
            skill: Some("mr-review".to_string()),
            cwd: Some(root.clone()),
            ..Default::default()
        })
        .unwrap();

        let args = command_target_args(&ctx);
        assert!(
            args.contains(&format!("--skill-dir {}", artifact_path(&skill_dir))),
            "selector keeps --skill-dir: {args}"
        );
        assert!(args.contains("--skill mr-review"), "{args}");
    }

    /// A run prepared with `--harness-file` must re-emit the flag in every
    /// generated follow-up command, or the follow-up silently resolves a
    /// different descriptor (#294).
    #[test]
    fn target_args_reemit_harness_file() {
        let tmp = TempDir::new().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let skill_subdir = make_skill(&root, "skills", "mr-review");
        let harness_file = root.join("overlay.toml");
        fs::write(
            &harness_file,
            r#"label = "claude-code"
"#,
        )
        .unwrap();

        let ctx = detect_run_context(DetectInput {
            cwd: Some(skill_subdir),
            harness_file: Some(harness_file.clone()),
            ..Default::default()
        })
        .unwrap();

        let args = command_target_args(&ctx);
        assert!(
            args.contains(&format!("--harness-file {}", artifact_path(&harness_file))),
            "selector re-emits --harness-file: {args}"
        );
    }

    /// The human runs `ingest`/`finalize` from a per-`(group, condition)` env dir.
    /// Without an explicit workspace root those commands default `workspace_root`
    /// to the derived eval home and bail "not found", so the selector must carry an
    /// absolute `--workspace-dir` pointing at the real workspace above the env.
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
            args.contains(&format!(
                "--workspace-dir {}",
                artifact_path(&ctx.workspace_root)
            )),
            "selector names absolute --workspace-dir: {args}"
        );
        assert!(
            ctx.workspace_root.is_absolute(),
            "workspace_root is absolute: {}",
            ctx.workspace_root.display()
        );

        // Round-trip from an env-like cwd below the workspace: feeding the
        // selector's roots back resolves the SAME workspace, not
        // the derived eval home.
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
}
