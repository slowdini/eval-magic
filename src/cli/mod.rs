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
use crate::workspace;

mod run;

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

/// `snapshot` adds a label and an optional git ref on top of the common set.
#[derive(Debug, Args)]
pub struct SnapshotArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Label for the snapshot (its directory name under `snapshots/`).
    #[arg(long)]
    pub label: Option<String>,
    /// Snapshot the skill as it existed at this git ref instead of the working
    /// tree. (`ref` is a Rust keyword, so the field is `reference`.)
    #[arg(long = "ref")]
    pub reference: Option<String>,
}

/// `promote-baseline` adds provenance flags (label + operator-declared models)
/// on top of the common set.
#[derive(Debug, Args)]
pub struct PromoteBaselineArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Provenance label recorded in `BASELINE.md`.
    #[arg(long)]
    pub label: Option<String>,
    /// Operator-declared agent model, recorded in `BASELINE.md`.
    #[arg(long)]
    pub agent_model: Option<String>,
    /// Operator-declared judge model, recorded in `BASELINE.md`.
    #[arg(long)]
    pub judge_model: Option<String>,
}

/// `run` adds the build-time flags (mode/baseline selection, staging toggles,
/// guard, plan-mode, bootstrap) on top of the common set.
#[derive(Debug, Args)]
pub struct RunArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Baseline snapshot label (required in `--mode revision`).
    #[arg(long)]
    pub baseline: Option<String>,
    /// SessionStart-equivalent bootstrap file inlined into each dispatch.
    #[arg(long)]
    pub bootstrap: Option<String>,
    /// Build the workspace but skip guard install and stop before next steps.
    #[arg(long)]
    pub dry_run: bool,
    /// Inline each condition's SKILL.md into the dispatch prompt instead of
    /// staging it under the harness skills dir.
    #[arg(long)]
    pub no_stage: bool,
    /// Arm the write guard (PreToolUse hook) for the dispatch window.
    #[arg(long)]
    pub guard: bool,
    /// Stage the skill-under-test under this verbatim name instead of the
    /// conspicuous `slow-powers-eval-…` slug.
    #[arg(long)]
    pub stage_name: Option<String>,
    /// Inject the harness's plan-mode profile as an operating-context layer.
    #[arg(long)]
    pub plan_mode: bool,
}

/// Every subcommand ported from eval-runner. Names match the original CLI.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Build dispatches and run evals (the default action).
    Run(RunArgs),
    /// Snapshot a workspace baseline.
    Snapshot(SnapshotArgs),
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
    PromoteBaseline(PromoteBaselineArgs),
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
    }));

    match command {
        Commands::Run(args) => run_run(args),
        Commands::Ingest(args) => run_ingest(args),
        Commands::Finalize(args) => run_finalize(args),
        Commands::Validate(args) => run_validate(args),
        Commands::TeardownGuard(_) => run_teardown_guard(),
        Commands::Guard { marker } => run_guard(marker),
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
    run_context_with_bootstrap(args, None)
}

/// Like [`run_context_from`], but threads an optional `--bootstrap` file (only
/// the `run` orchestrator consumes it; post-dispatch stages pass `None`).
fn run_context_with_bootstrap(
    args: &CommonArgs,
    bootstrap: Option<String>,
) -> anyhow::Result<RunContext> {
    Ok(detect_run_context(DetectInput {
        skill_dir: args.skill_dir.clone(),
        skill: args.skill.clone(),
        bootstrap,
        workspace_dir: args.workspace_dir.clone(),
        harness: args.harness,
    })?)
}

/// Split a comma-separated `--only`/`--skip` value into trimmed, non-empty ids.
fn parse_id_list(v: Option<&str>) -> Option<Vec<String>> {
    v.map(|s| {
        s.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    })
}

/// Build the iteration workspace and dispatch plan (the default action). Ports
/// eval-runner's `commandRun`.
fn run_run(args: RunArgs) -> anyhow::Result<()> {
    let ctx = run_context_with_bootstrap(&args.common, args.bootstrap.clone())?;
    let only = parse_id_list(args.common.only.as_deref());
    let skip = parse_id_list(args.common.skip.as_deref());
    run::orchestrate::command_run(
        &ctx,
        &run::orchestrate::RunOptions {
            mode: args.common.mode.as_deref(),
            baseline: args.baseline.as_deref(),
            only: only.as_deref(),
            skip: skip.as_deref(),
            iteration: args.common.iteration,
            dry_run: args.dry_run,
            no_stage: args.no_stage,
            guard: args.guard,
            stage_name: args.stage_name.as_deref(),
            plan_mode: args.plan_mode,
        },
    )?;
    Ok(())
}

/// Execute one chain step by mapping its [`run::steps::StepKind`] to the stage
/// handler. This is the production runner for [`run::steps::run_steps`]; it
/// prints the `error: <msg>` contract on failure (matching eval-runner's default
/// runner) before propagating, so the chain's halt-and-retry message still fires.
fn run_step(step: &run::steps::StepCommand) -> anyhow::Result<()> {
    use run::steps::StepKind;
    let common = CommonArgs {
        skill_dir: Some(step.skill_dir.clone()),
        skill: Some(step.skill.clone()),
        iteration: Some(step.iteration),
        mode: None,
        harness: Some(step.harness),
        workspace_dir: step.workspace_dir.clone(),
        subagents_dir: step.subagents_dir.clone(),
        only: None,
        skip: None,
        overwrite: false,
    };
    let result = match step.kind {
        StepKind::RecordRuns => run_record_runs(common),
        StepKind::FillTranscripts => run_fill_transcripts(common),
        StepKind::DetectStrayWrites => run_detect_stray_writes(common),
        StepKind::Grade { finalize } => run_grade(GradeArgs { common, finalize }),
        StepKind::Aggregate => run_aggregate(common),
    };
    if let Err(e) = &result {
        eprintln!("error: {e:#}");
    }
    result
}

/// Run the post-dispatch chain (record-runs → fill-transcripts →
/// detect-stray-writes → grade) and stop at the judge hand-off. Ports
/// eval-runner's `commandIngest`.
fn run_ingest(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let iteration = args
        .iteration
        .ok_or_else(|| anyhow!("ingest requires --iteration <N>"))?;
    if ctx.harness == Harness::ClaudeCode && args.subagents_dir.is_none() {
        bail!(
            "ingest requires --subagents-dir <path> (Claude Code persists subagent transcripts under ~/.claude/projects/<project-slug>/<parent-session-id>/subagents/)"
        );
    }

    let steps = run::steps::build_ingest_commands(&run::steps::StepParams {
        skill_dir: args.skill_dir.as_deref().unwrap_or_default(),
        skill: args.skill.as_deref().unwrap_or_default(),
        iteration,
        harness: ctx.harness,
        subagents_dir: args.subagents_dir.as_deref(),
        workspace_dir: args.workspace_dir.as_deref(),
    });
    if let Some(failed) = run::steps::run_steps(&steps, run_step) {
        bail!(
            "ingest stopped at '{failed}'. Fix the failure and re-run ingest — completed steps skip work that's already done."
        );
    }

    let judge_path = ctx
        .workspace_root
        .join(&ctx.skill_name)
        .join(format!("iteration-{iteration}"))
        .join("judge-tasks.json");
    let total_tasks = std::fs::read_to_string(&judge_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("total_tasks").and_then(serde_json::Value::as_u64));
    match total_tasks {
        Some(0) => println!(
            "\n✅ Ingest complete — no judge dispatches needed.\nNext: skill-eval finalize --skill {} --iteration {iteration}",
            ctx.skill_name
        ),
        Some(n) => println!(
            "\n✅ Ingest complete. Dispatch the {n} judge task(s) grade listed above (judge-tasks.json), then:\n  skill-eval finalize --skill {} --iteration {iteration}",
            ctx.skill_name
        ),
        None => println!(
            "\n✅ Ingest complete. Dispatch the judge task(s) grade listed above (judge-tasks.json), then:\n  skill-eval finalize --skill {} --iteration {iteration}",
            ctx.skill_name
        ),
    }
    Ok(())
}

/// Run the post-judge chain (grade --finalize → aggregate). Ports eval-runner's
/// `commandFinalize`.
fn run_finalize(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let iteration = args
        .iteration
        .ok_or_else(|| anyhow!("finalize requires --iteration <N>"))?;

    let steps = run::steps::build_finalize_commands(&run::steps::StepParams {
        skill_dir: args.skill_dir.as_deref().unwrap_or_default(),
        skill: args.skill.as_deref().unwrap_or_default(),
        iteration,
        harness: ctx.harness,
        subagents_dir: None,
        workspace_dir: args.workspace_dir.as_deref(),
    });
    if let Some(failed) = run::steps::run_steps(&steps, run_step) {
        bail!("finalize stopped at '{failed}'. Fix the failure and re-run finalize.");
    }
    println!(
        "\n✅ Finalize complete. Read the benchmark above, then tear down: skill-eval teardown --skill {}",
        ctx.skill_name
    );
    Ok(())
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

/// Snapshot the skill (`SKILL.md` + sibling assets, excluding `evals/`) into
/// `<workspace>/<skill>/snapshots/<label>/`, from the working tree or — with
/// `--ref` — a git ref. Ports eval-runner's `commandSnapshot`.
fn run_snapshot(args: SnapshotArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args.common)?;
    let label = args
        .label
        .ok_or_else(|| anyhow!("snapshot requires --label <name>"))?;
    let reference = args.reference.as_deref();

    let dest = workspace::snapshot(
        &ctx.workspace_root,
        &ctx.skill_name,
        &ctx.skill_subdir,
        &label,
        reference,
    )?;

    match reference {
        Some(reference) => println!(
            "Snapshotted {} at {reference} → {}",
            ctx.skill_name,
            dest.display()
        ),
        None => println!("Snapshotted {} → {}", ctx.skill_name, dest.display()),
    }
    Ok(())
}

/// Promote an iteration's `benchmark.json` + per-run gradings into the skill's
/// committed `evals/baseline/`, dropping a `.promoted.json` marker. Ports
/// eval-runner's `promote-baseline` `main`.
fn run_promote_baseline(args: PromoteBaselineArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args.common)?;
    let iteration = args
        .common
        .iteration
        .ok_or_else(|| anyhow!("missing --iteration <N>"))?;

    let result = workspace::promote_baseline(&workspace::PromoteOptions {
        workspace_root: &ctx.workspace_root,
        skill_name: &ctx.skill_name,
        skill_subdir: &ctx.skill_subdir,
        iteration,
        harness: ctx.harness,
        label: args.label.as_deref(),
        agent_model: args.agent_model.as_deref(),
        judge_model: args.judge_model.as_deref(),
        git_cwd: &ctx.skill_subdir,
    })?;

    let n = result.gradings_copied;
    println!(
        "Promoted baseline for {} → {} (benchmark.json + {n} grading file{} + BASELINE.md)",
        ctx.skill_name,
        result.baseline_dir.display(),
        if n == 1 { "" } else { "s" }
    );
    Ok(())
}

/// End-of-run teardown: disarm the write guard, remove the staged skill set (and
/// prune a `.claude` the runner emptied), then reclaim the workspace, preserving
/// any iteration with uncommitted results. Ports eval-runner's `teardown`
/// command.
fn run_teardown(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    // The guard lives at `<cwd>/.claude` (cwd-only, matching `teardown-guard`).
    let torn = sandbox::teardown_guard(&std::env::current_dir()?);
    run::staging::cleanup_staged_skills(&ctx.stage_root, ctx.harness)?;
    let ws = workspace::cleanup_workspace(&ctx.workspace_root, &ctx.skill_name);

    println!(
        "🧹 Eval teardown complete: staged skill set removed{}.",
        if torn {
            " and write guard disarmed"
        } else {
            ""
        }
    );
    let reclaimed = ws.removed_iterations.len() + ws.removed_snapshots.len();
    if reclaimed > 0 {
        println!(
            "   Reclaimed {} workspace iteration(s) and {} reproducible snapshot(s).",
            ws.removed_iterations.len(),
            ws.removed_snapshots.len()
        );
    }
    if !ws.kept_iterations.is_empty() {
        let lines = ws
            .kept_iterations
            .iter()
            .map(|k| format!("     - {} ({})", k.iteration, k.reason))
            .collect::<Vec<_>>()
            .join("\n");
        eprintln!(
            "⚠ Kept {} workspace iteration(s) with results not yet committed:\n{lines}\n   Commit them, e.g.:\n     skill-eval promote-baseline --skill {} --iteration <N>\n   or delete {}/ manually to discard.",
            ws.kept_iterations.len(),
            ctx.skill_name,
            Path::new("skills-workspace")
                .join(&ctx.skill_name)
                .display()
        );
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
