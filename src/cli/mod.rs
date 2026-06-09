//! CLI surface: command-tree definition and dispatch.
//!
//! Mirrors `src/cli/` in eval-runner (`cli.ts`, `run.ts`, `help.ts`). The
//! manual flag parsing and hand-written help of the original are replaced by a
//! `clap` derive tree, so `help.ts` has no counterpart here.
//!
//! The full subcommand surface is fixed here up front; each handler reports
//! "not yet implemented" until its owning module is ported (see
//! rewrite-roadmap.md). Flags are intentionally permissive (mostly optional)
//! during the port and are tightened per-command as behavior lands.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};
use clap::{Args, Parser, Subcommand};

use crate::core::{DetectInput, Harness, RunContext, detect_run_context};
use crate::pipeline;
use crate::sandbox;
use crate::validation;

/// Top-level CLI. With no subcommand, the default action is `run`.
#[derive(Debug, Parser)]
#[command(
    name = "skill-eval",
    version,
    about = "Run skill evals — measure whether an agent skill actually shifts behavior."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

/// Flags shared by most subcommands. Ported from the manual `flag()` parsing in
/// eval-runner's `run.ts`/`cli.ts`.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Directory containing the skill under evaluation.
    #[arg(long)]
    pub skill_dir: Option<String>,
    /// Skill name under evaluation.
    #[arg(long)]
    pub skill: Option<String>,
    /// Iteration number for post-dispatch steps.
    #[arg(long)]
    pub iteration: Option<u32>,
    /// Comparison mode: `new-skill` (with vs. without) or `revision` (old vs. new).
    #[arg(long)]
    pub mode: Option<String>,
    /// Target harness.
    #[arg(long)]
    pub harness: Option<Harness>,
    /// Workspace directory (defaults to `<cwd>/skills-workspace`).
    #[arg(long)]
    pub workspace_dir: Option<String>,
    /// Subagents transcript dir (Claude Code only), e.g.
    /// `~/.claude/projects/<slug>/<session-id>/subagents/`.
    #[arg(long)]
    pub subagents_dir: Option<String>,
    /// Restrict to these eval ids (comma-separated).
    #[arg(long)]
    pub only: Option<String>,
    /// Skip these eval ids (comma-separated).
    #[arg(long)]
    pub skip: Option<String>,
    /// Replace existing records rather than erroring.
    #[arg(long)]
    pub overwrite: bool,
}

/// `validate` only needs to know where to look.
#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Directory whose `evals.json` files should be validated.
    #[arg(long)]
    pub skill_dir: Option<String>,
}

/// `grade` adds a finalize flag on top of the common set.
#[derive(Debug, Args)]
pub struct GradeArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Merge judge responses instead of emitting judge tasks.
    #[arg(long)]
    pub finalize: bool,
}

/// Every subcommand ported from eval-runner. Names match the original CLI.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Build dispatches and run evals (the default action).
    Run(CommonArgs),
    /// Snapshot a workspace baseline.
    Snapshot(CommonArgs),
    /// Tear down a workspace.
    Teardown(CommonArgs),
    /// Disarm the write guard.
    TeardownGuard(CommonArgs),
    /// Ingest recorded transcripts into run records.
    Ingest(CommonArgs),
    /// Finalize grading after judge responses are in.
    Finalize(CommonArgs),
    /// Assemble run records from a dispatch and its transcripts.
    RecordRuns(CommonArgs),
    /// Populate tool invocations from persisted transcripts.
    FillTranscripts(CommonArgs),
    /// Detect writes outside the sandbox output boundary.
    DetectStrayWrites(CommonArgs),
    /// Grade run records (transcript checks + LLM-judge task emission).
    Grade(GradeArgs),
    /// Aggregate before/after benchmark deltas.
    Aggregate(CommonArgs),
    /// Promote a benchmark + gradings into a committed baseline.
    PromoteBaseline(CommonArgs),
    /// Validate `evals.json` files against the bundled schemas.
    Validate(ValidateArgs),
    /// Internal PreToolUse hook entry point. Invoked by the installed write-guard
    /// hook as `skill-eval guard <marker>`, not by users; hidden from help.
    #[command(hide = true)]
    Guard {
        /// Path to the guard marker file. Defaults to
        /// `<cwd>/.claude/skills/.slow-powers-eval-guard.json`.
        marker: Option<String>,
    },
}

/// Parse process arguments, dispatch to the selected subcommand, and return its
/// result. Called by the binary entry point.
pub fn run() -> anyhow::Result<()> {
    dispatch(Cli::parse().command)
}

fn dispatch(command: Option<Commands>) -> anyhow::Result<()> {
    // No subcommand means the default `run` action.
    let command = command.unwrap_or(Commands::Run(CommonArgs {
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
    }));

    match command {
        Commands::Validate(args) => run_validate(args),
        Commands::TeardownGuard(_) => run_teardown_guard(),
        Commands::Guard { marker } => run_guard(marker),
        Commands::RecordRuns(args) => run_record_runs(args),
        Commands::FillTranscripts(args) => run_fill_transcripts(args),
        Commands::DetectStrayWrites(args) => run_detect_stray_writes(args),
        Commands::Grade(args) => run_grade(args),
        Commands::Aggregate(args) => run_aggregate(args),
        other => {
            let name = match other {
                Commands::Run(_) => "run",
                Commands::Snapshot(_) => "snapshot",
                Commands::Teardown(_) => "teardown",
                Commands::Ingest(_) => "ingest",
                Commands::Finalize(_) => "finalize",
                Commands::PromoteBaseline(_) => "promote-baseline",
                Commands::Validate(_)
                | Commands::TeardownGuard(_)
                | Commands::Guard { .. }
                | Commands::RecordRuns(_)
                | Commands::FillTranscripts(_)
                | Commands::DetectStrayWrites(_)
                | Commands::Grade(_)
                | Commands::Aggregate(_) => {
                    unreachable!("handled above")
                }
            };
            bail!("`{name}` is not yet implemented (see rewrite-roadmap.md)");
        }
    }
}

/// Validate every `<skill>/evals/evals.json` under `--skill-dir`, printing a
/// `✓`/`✗` line per file and a summary. Exits non-zero if any file failed.
/// Ports eval-runner's `validate-all.ts` `main`.
fn run_validate(args: ValidateArgs) -> anyhow::Result<()> {
    let skill_dir = args
        .skill_dir
        .ok_or_else(|| anyhow::anyhow!("missing required flag --skill-dir <path>"))?;
    let skill_dir = Path::new(&skill_dir);
    if !skill_dir.is_dir() {
        bail!("skills dir not found: {}", skill_dir.display());
    }

    let report = validation::validate_all(skill_dir)?;
    for outcome in &report.outcomes {
        match &outcome.error {
            None => println!("✓ {}", outcome.rel_path),
            Some(message) => eprintln!("✗ {message}"),
        }
    }
    println!(
        "\nValidated {} evals.json file(s); {} failed.",
        report.validated(),
        report.failed()
    );

    if report.failed() > 0 {
        let details = report
            .outcomes
            .iter()
            .filter_map(|o| o.error.as_ref().map(|m| format!("  - {}: {m}", o.skill)))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "{} evals.json file(s) failed validation:\n{details}",
            report.failed()
        );
    }

    Ok(())
}

/// Resolve a [`RunContext`] from the shared flags (skill dir/name, workspace,
/// harness). Used by every post-dispatch stage handler.
fn run_context_from(args: &CommonArgs) -> anyhow::Result<RunContext> {
    Ok(detect_run_context(DetectInput {
        skill_dir: args.skill_dir.clone(),
        skill: args.skill.clone(),
        bootstrap: None,
        workspace_dir: args.workspace_dir.clone(),
        harness: args.harness,
    })?)
}

/// The iteration directory for a run: `<workspace>/<skill>/iteration-<n>`. Errors
/// if `--iteration` is absent or the directory does not exist.
fn iteration_dir(ctx: &RunContext, iteration: Option<u32>) -> anyhow::Result<PathBuf> {
    let iteration = iteration.ok_or_else(|| anyhow!("missing --iteration"))?;
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
fn check_subagents_dir(harness: Harness, subagents_dir: Option<&Path>) -> anyhow::Result<()> {
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

/// Assemble `run.json` + `timing.json` for every task in the iteration's
/// `dispatch.json`. Ports eval-runner's `record-runs` `main`.
fn run_record_runs(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let subagents_dir = args.subagents_dir.as_deref().map(Path::new);
    check_subagents_dir(ctx.harness, subagents_dir)?;

    let dir = iteration_dir(&ctx, args.iteration)?;
    let result = pipeline::record_runs(&dir, ctx.harness, subagents_dir, args.overwrite)?;

    println!(
        "\nRecorded: {}, skipped (existing run.json): {}, skipped (no final message): {}, missing transcript: {}",
        result.recorded,
        result.skipped_existing,
        result.skipped_no_final_message,
        result.missing_transcript
    );
    Ok(())
}

/// Populate `tool_invocations` from persisted transcripts for every `run.json` in
/// the iteration. Ports eval-runner's `fill-transcripts` `main`.
fn run_fill_transcripts(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let subagents_dir = args.subagents_dir.as_deref().map(Path::new);
    check_subagents_dir(ctx.harness, subagents_dir)?;

    let dir = iteration_dir(&ctx, args.iteration)?;
    let result = pipeline::fill_transcripts(&dir, ctx.harness, subagents_dir, args.overwrite)?;

    println!(
        "\nFilled: {}, skipped (already populated): {}, missing transcript: {}",
        result.filled, result.skipped, result.missing
    );
    Ok(())
}

/// Report writes outside the sandbox output boundary (and live-source reads) for
/// every run in the iteration. Ports eval-runner's `detect-stray-writes` `main`.
fn run_detect_stray_writes(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let iteration = args
        .iteration
        .ok_or_else(|| anyhow!("missing --iteration"))?;
    let dir = iteration_dir(&ctx, Some(iteration))?;
    let repo_root = std::env::current_dir()?;

    let report =
        pipeline::detect_stray_writes_report(&dir, iteration, &ctx.skill_subdir, &repo_root)?;
    println!("Wrote {}", dir.join("stray-writes.json").display());

    for r in &report.runs {
        for v in &r.violations {
            eprintln!(
                "✗ {}/{}: {} wrote outside outputs dir → {} (ordinal {})",
                r.eval_id,
                r.condition,
                v.tool,
                v.path.as_deref().unwrap_or(""),
                v.ordinal
            );
        }
        for w in &r.warnings {
            eprintln!(
                "⚠ {}/{}: Bash {} (ordinal {}): {}",
                r.eval_id,
                r.condition,
                w.reason,
                w.ordinal,
                w.command.as_deref().unwrap_or("")
            );
        }
        for l in &r.live_source_reads {
            eprintln!(
                "⚠ {}/{}: {} read the live skill source (ordinal {}): {}",
                r.eval_id,
                r.condition,
                l.tool,
                l.ordinal,
                l.path.as_deref().or(l.command.as_deref()).unwrap_or("")
            );
        }
    }

    let t = report.totals;
    if t.violations == 0 && t.warnings == 0 && t.live_source_reads == 0 {
        println!("✓ No out-of-bounds writes or live-source reads detected.");
    } else {
        eprintln!(
            "\n{} violation(s), {} warning(s), {} live-source read(s). Runs with violations edited files outside their sandbox; runs with live-source reads saw the live skill instead of their staged copy — treat those data points as tainted.",
            t.violations, t.warnings, t.live_source_reads
        );
    }
    Ok(())
}

/// Grade run records. Default mode emits LLM judge tasks (+ the skill-invocation
/// meta-check); `--finalize` folds judge responses into `grading.json`. Ports
/// eval-runner's `grade` `main`.
fn run_grade(args: GradeArgs) -> anyhow::Result<()> {
    let common = args.common;
    let ctx = run_context_from(&common)?;
    let iteration = common
        .iteration
        .ok_or_else(|| anyhow!("missing --iteration"))?;
    let dir = iteration_dir(&ctx, Some(iteration))?;

    let conditions_path = dir.join("conditions.json");
    if !conditions_path.exists() {
        bail!("missing: {}", conditions_path.display());
    }
    let conditions: crate::core::ConditionsRecord =
        serde_json::from_str(&std::fs::read_to_string(&conditions_path)?)?;

    let evals_path = ctx.skill_subdir.join("evals").join("evals.json");
    let evals_value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&evals_path)?)?;
    let evals = validation::validate_evals_config(&evals_value, &evals_path.to_string_lossy())?;

    let gctx = pipeline::GradeContext {
        iteration_dir: &dir,
        conditions: &conditions,
        evals: &evals,
    };

    if args.finalize {
        let s = pipeline::finalize(&gctx)?;
        println!(
            "\nFinalized: {} substantive assertion(s) graded, {} skill-invocation meta-check(s) graded, {} transcript_check unverifiable (empty tool_invocations).",
            s.total_graded, s.total_meta_graded, s.total_unverifiable
        );
        if s.meta_failures > 0 {
            eprintln!(
                "\n⚠ {} run(s) failed the skill-invocation meta-check. Substantive results for those runs may be unreliable.",
                s.meta_failures
            );
        }
        println!(
            "\nNext: skill-eval aggregate --skill {} --iteration {iteration}",
            ctx.skill_name
        );
    } else {
        let s = pipeline::emit_judge_tasks(&gctx)?;
        println!("Wrote {}", dir.join("judge-tasks.json").display());
        println!(
            "Judge tasks: {} ({} skill-invocation meta-judge(s))",
            s.total_tasks, s.meta_injected
        );
        if s.meta_code_checked > 0 {
            println!(
                "Skill-invocation code-checked: {} (transcript-based, no judge needed)",
                s.meta_code_checked
            );
        }
        println!(
            "\nNext: dispatch each task as a judge subagent, write each verdict to its `response_path`, then run: skill-eval grade --skill {} --iteration {iteration} --finalize",
            ctx.skill_name
        );
    }
    Ok(())
}

/// Compute before/after benchmark deltas across the two conditions. Ports
/// eval-runner's `aggregate` `main`.
fn run_aggregate(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let dir = iteration_dir(&ctx, args.iteration)?;

    let conditions_path = dir.join("conditions.json");
    if !conditions_path.exists() {
        bail!("missing: {}", conditions_path.display());
    }
    let conditions: crate::core::ConditionsRecord =
        serde_json::from_str(&std::fs::read_to_string(&conditions_path)?)?;

    let benchmark = pipeline::aggregate(&dir, &conditions)?;
    println!("Wrote {}", dir.join("benchmark.json").display());
    if benchmark.missing_gradings > 0 {
        eprintln!(
            "note: {} grading.json file(s) were missing — benchmark is incomplete.",
            benchmark.missing_gradings
        );
    }
    for w in &benchmark.validity_warnings {
        eprintln!("⚠ {w}");
    }
    Ok(())
}

/// Disarm the write guard for the current directory. Ports eval-runner's
/// `teardown-guard` command, but cwd-only: the guard lives at `<cwd>/.claude`, so
/// (unlike the TS original) this needs no `--skill-dir`/`--skill` flags.
fn run_teardown_guard() -> anyhow::Result<()> {
    let torn = sandbox::teardown_guard(&std::env::current_dir()?);
    println!(
        "{}",
        if torn {
            "🛡 Write guard removed."
        } else {
            "No write guard was installed — nothing to remove."
        }
    );
    Ok(())
}

/// The hidden PreToolUse hook entry point. Reads the hook payload from stdin and
/// the marker path from argv, then prints a deny verdict for out-of-bounds calls.
/// Ports eval-runner's `guard.ts`: it **fails open** — every error path allows the
/// call and exits 0, so the guard can never brick a session.
fn run_guard(marker: Option<String>) -> anyhow::Result<()> {
    let marker_path = marker
        .map(PathBuf::from)
        .unwrap_or_else(default_marker_path);
    let payload = io::read_to_string(io::stdin()).unwrap_or_default();
    if let Some(verdict) = sandbox::guard_decision(&payload, sandbox::read_marker(&marker_path)) {
        print!("{verdict}");
    }
    Ok(())
}

/// The marker path the guard reads when argv carries none:
/// `<cwd>/.claude/skills/.slow-powers-eval-guard.json`.
fn default_marker_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_default()
        .join(".claude")
        .join("skills")
        .join(sandbox::GUARD_MARKER)
}
