//! The post-dispatch / post-judge pipeline command handlers: the `ingest` and
//! `finalize` chains and each individual stage (`record-runs`,
//! `fill-transcripts`, `detect-stray-writes`, `grade`, `aggregate`).

use std::path::Path;

use anyhow::{anyhow, bail};

use crate::cli::args::{CommonArgs, GradeArgs};
use crate::cli::run;
use crate::cli::{check_subagents_dir, iteration_dir, run_context_from};
use crate::core::Harness;
use crate::pipeline;
use crate::validation;

/// Execute one chain step by mapping its [`run::steps::StepKind`] to the stage
/// handler. This is the production runner for [`run::steps::run_steps`]; it
/// prints the `error: <msg>` contract on failure before propagating, so the
/// chain's halt-and-retry message still fires.
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
/// detect-stray-writes → grade) and stop at the judge hand-off.
pub(crate) fn run_ingest(args: CommonArgs) -> anyhow::Result<()> {
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
            "\n✅ Ingest complete — no judge dispatches needed.\nNext: eval-magic finalize --skill {} --iteration {iteration}",
            ctx.skill_name
        ),
        Some(n) => println!(
            "\n✅ Ingest complete. Dispatch the {n} judge task(s) grade listed above (judge-tasks.json), then:\n  eval-magic finalize --skill {} --iteration {iteration}",
            ctx.skill_name
        ),
        None => println!(
            "\n✅ Ingest complete. Dispatch the judge task(s) grade listed above (judge-tasks.json), then:\n  eval-magic finalize --skill {} --iteration {iteration}",
            ctx.skill_name
        ),
    }
    Ok(())
}

/// Run the post-judge chain (grade --finalize → aggregate).
pub(crate) fn run_finalize(args: CommonArgs) -> anyhow::Result<()> {
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
        "\n✅ Finalize complete. Read the benchmark above, then tear down: eval-magic teardown --skill {}",
        ctx.skill_name
    );
    Ok(())
}

/// Assemble `run.json` + `timing.json` for every task in the iteration's
/// `dispatch.json`.
pub(crate) fn run_record_runs(args: CommonArgs) -> anyhow::Result<()> {
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
/// the iteration.
pub(crate) fn run_fill_transcripts(args: CommonArgs) -> anyhow::Result<()> {
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
/// every run in the iteration.
pub(crate) fn run_detect_stray_writes(args: CommonArgs) -> anyhow::Result<()> {
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
/// meta-check); `--finalize` folds judge responses into `grading.json`.
pub(crate) fn run_grade(args: GradeArgs) -> anyhow::Result<()> {
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
            "\nNext: eval-magic aggregate --skill {} --iteration {iteration}",
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
            "\nNext: dispatch each task as a judge subagent, write each verdict to its `response_path`, then run: eval-magic grade --skill {} --iteration {iteration} --finalize",
            ctx.skill_name
        );
    }
    Ok(())
}

/// Compute before/after benchmark deltas across the two conditions.
pub(crate) fn run_aggregate(args: CommonArgs) -> anyhow::Result<()> {
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
