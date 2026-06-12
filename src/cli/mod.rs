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

/// Render the shortest target selector that will resolve the same skill from
/// the current command context.
pub(crate) fn command_target_args(ctx: &RunContext) -> String {
    if ctx.stage_siblings {
        return format!(
            " --skill-dir {} --skill {}",
            ctx.skill_dir.display(),
            ctx.skill_name
        );
    }
    if ctx.stage_root == ctx.skill_subdir {
        String::new()
    } else {
        format!(" --skill {}", ctx.skill_subdir.display())
    }
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

/// Claude Code reads subagent transcripts by description, so `--subagents-dir` is
/// required and must exist; Codex reads `outputs/codex-events.jsonl`, so it's
/// ignored there.
pub(crate) fn check_subagents_dir(
    harness: Harness,
    subagents_dir: Option<&Path>,
) -> anyhow::Result<()> {
    if harness == Harness::ClaudeCode {
        match subagents_dir {
            None => bail!(
                "missing --subagents-dir (e.g. ~/.claude/projects/<project-slug>/<parent-session-id>/subagents/)"
            ),
            Some(dir) if !dir.exists() => bail!("subagents-dir not found: {}", dir.display()),
            Some(_) => {}
        }
    }
    Ok(())
}
