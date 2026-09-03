//! The post-dispatch / post-judge pipeline command handlers: the `ingest` and
//! `finalize` chains and each individual stage (`record-runs`,
//! `detect-stray-writes`, `grade`, `aggregate`).

use anyhow::bail;

use crate::cli::args::{CommonArgs, GradeArgs};
use crate::cli::command_target_args;
use crate::cli::run;
use crate::cli::{
    harness_descriptor_drift_warning, iteration_dir, resolve_iteration, run_context_from,
    staged_env_roots,
};
use crate::core::RunContext;
use crate::pipeline;
use crate::sandbox;
use std::path::{Path, PathBuf};

/// The command that dispatches the judge tasks `ingest` emitted. Harness-
/// independent: the runner drives judges the same way it drives eval tasks, so
/// the only thing that varies is the `--harness` selector.
fn judge_dispatch_guidance(ctx: &RunContext, iteration: u32) -> String {
    format!(
        "eval-magic dispatch --judges{} --iteration {iteration} --harness {}",
        command_target_args(ctx),
        ctx.harness.name()
    )
}

/// Execute one chain step by mapping its [`run::steps::StepKind`] to the stage
/// handler. This is the production runner for [`run::steps::run_steps`]; it
/// prints the `error: <msg>` contract on failure before propagating, so the
/// chain's halt-and-retry message still fires.
fn run_step(step: &run::steps::StepCommand) -> anyhow::Result<()> {
    use run::steps::StepKind;
    let common = CommonArgs {
        skill_dir: step.skill_dir.clone(),
        skill: step.skill.clone(),
        iteration: Some(step.iteration),
        mode: None,
        harness: Some(step.harness.name().to_string()),
        workspace_dir: step.workspace_dir.clone(),
        only: None,
        skip: None,
        overwrite: false,
    };
    let result = match step.kind {
        StepKind::RecordRuns => run_record_runs(common),
        StepKind::DetectStrayWrites => run_detect_stray_writes(common),
        StepKind::Grade { finalize } => run_grade(GradeArgs { common, finalize }),
        StepKind::Aggregate => run_aggregate(common),
    };
    if let Err(e) = &result {
        eprintln!("error: {e:#}");
    }
    result
}

/// Run the post-dispatch chain (record-runs → detect-stray-writes → grade) and
/// stop at the judge hand-off.
pub(crate) fn run_ingest(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let iteration = resolve_iteration(&ctx, args.iteration)?;
    let dir = ctx
        .workspace_root
        .join(&ctx.skill_name)
        .join(format!("iteration-{iteration}"));
    if let Some(warning) = harness_descriptor_drift_warning(&ctx, &dir) {
        eprintln!("⚠ {warning}");
    }

    let steps = run::steps::build_ingest_commands(&run::steps::StepParams {
        skill_dir: args.skill_dir.as_deref(),
        skill: args.skill.as_deref(),
        iteration,
        harness: ctx.harness,
        workspace_dir: args.workspace_dir.as_deref(),
    });
    if let Some(failed) = run::steps::run_steps(&steps, run_step) {
        bail!(
            "ingest stopped at '{failed}'. Fix the failure and re-run ingest — completed steps skip work that's already done."
        );
    }

    let judge_path = dir.join("judge-tasks.json");
    let total_tasks = std::fs::read_to_string(&judge_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("total_tasks").and_then(serde_json::Value::as_u64));
    let target_args = command_target_args(&ctx);
    let judge_guidance = judge_dispatch_guidance(&ctx, iteration);
    match total_tasks {
        Some(0) => println!(
            "\n✅ Ingest complete — no judge dispatches needed.\nNext: eval-magic finalize{target_args} --iteration {iteration}"
        ),
        Some(n) => println!(
            "\n✅ Ingest complete. {n} judge task(s) ready.\n{judge_guidance}\nThen run:\n  eval-magic finalize{target_args} --iteration {iteration}"
        ),
        None => println!(
            "\n✅ Ingest complete. Judge task(s) ready.\n{judge_guidance}\nThen run:\n  eval-magic finalize{target_args} --iteration {iteration}"
        ),
    }
    Ok(())
}

/// Run the post-judge chain (grade --finalize → aggregate).
pub(crate) fn run_finalize(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let iteration = resolve_iteration(&ctx, args.iteration)?;

    let steps = run::steps::build_finalize_commands(&run::steps::StepParams {
        skill_dir: args.skill_dir.as_deref(),
        skill: args.skill.as_deref(),
        iteration,
        harness: ctx.harness,
        workspace_dir: args.workspace_dir.as_deref(),
    });
    if let Some(failed) = run::steps::run_steps(&steps, run_step) {
        bail!("finalize stopped at '{failed}'. Fix the failure and re-run finalize.");
    }
    let target_args = command_target_args(&ctx);
    println!(
        "\n✅ Finalize complete. Read the benchmark above, then tear down: eval-magic teardown{target_args}"
    );
    // Warn if a guard is still armed. There is one env per (group, condition), so
    // walk each per-env marker as well as the cwd. The reminder names `teardown`
    // rather than `teardown-guard`: both disarm every one of these, but at end of
    // run the staged skill set and the workspace want reclaiming too.
    let mut armed = sandbox::guard_is_armed(&ctx.stage_root);
    if !armed && let Ok(dir) = iteration_dir(&ctx, Some(iteration)) {
        armed = staged_env_roots(&dir)
            .iter()
            .any(|env| sandbox::guard_is_armed(env));
    }
    if armed {
        println!(
            "⚠ Guard still armed — run `eval-magic teardown` to disarm before editing source."
        );
    }
    Ok(())
}

/// Assemble `run.json` + `timing.json` for every task in the iteration's
/// `dispatch.json`.
pub(crate) fn run_record_runs(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let iteration = resolve_iteration(&ctx, args.iteration)?;
    let dir = iteration_dir(&ctx, Some(iteration))?;
    let result = pipeline::record_runs(&dir, iteration, ctx.harness, args.overwrite)?;

    println!(
        "\nRecorded: {}, skipped (existing run.json): {}, skipped (no final response): {}, skipped (prompt unread): {}, skipped (missing completion artifact): {}, missing transcript: {}",
        result.recorded,
        result.skipped_existing,
        result.skipped_no_final_response,
        result.skipped_prompt_unread,
        result.skipped_incomplete_conversation,
        result.missing_transcript
    );
    if let Some(warning) = result.transcript_warning(ctx.harness) {
        eprintln!("{warning}");
    }
    if let Some(warning) = result.prompt_unread_warning() {
        eprintln!("{warning}");
    }
    if let Some(warning) = result.incomplete_conversation_warning() {
        eprintln!("{warning}");
    }
    if let Some(warning) = result.permission_denial_warning() {
        eprintln!("{warning}");
    }
    Ok(())
}

/// Report writes outside each private task environment (and live-source reads) for
/// every run in the iteration.
pub(crate) fn run_detect_stray_writes(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let iteration = resolve_iteration(&ctx, args.iteration)?;
    let dir = iteration_dir(&ctx, Some(iteration))?;
    let repo_root = std::env::current_dir()?;

    let report =
        pipeline::detect_stray_writes_report(&dir, iteration, &ctx.skill_subdir, &repo_root)?;
    println!("Wrote {}", dir.join("stray-writes.json").display());
    println!("Wrote {}", dir.join("guard-denials.json").display());

    for notice in &report.notices {
        eprintln!("⚠ {notice}");
    }

    if report.guard_denials > 0 {
        eprintln!(
            "⚠ {} guard denial(s) altered agent behavior — inspect guard-denials.json before \
             trusting the affected data points.",
            report.guard_denials
        );
    }

    for r in &report.runs {
        for v in &r.violations {
            eprintln!(
                "✗ {}/{}: {} wrote outside task environment → {} (ordinal {})",
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
    let clean = t.violations == 0 && t.warnings == 0 && t.live_source_reads == 0;
    if clean && report.invocations_inspected == 0 {
        eprintln!(
            "⚠ Unverifiable — 0 transcript tool-calls inspected. Stray-write detection had nothing to check (every run's tool_invocations is empty); link transcripts first, then re-run (confirm each task's `outputs/<harness>-events.jsonl` exists — see the record-runs warning)."
        );
    } else if clean {
        println!("✓ No out-of-bounds writes or live-source reads detected.");
    } else {
        eprintln!(
            "\n{} violation(s), {} warning(s), {} live-source read(s). Runs with violations edited files outside their sandbox; runs with live-source reads saw the live skill instead of their staged copy — treat those data points as tainted.",
            t.violations, t.warnings, t.live_source_reads
        );
    }
    Ok(())
}

/// The skill directory a post-dispatch phase reads its inputs from: the copy the
/// iteration holds, falling back to the live tree for iterations prepared before
/// skills were sourced.
///
/// The eval definitions that describe what ran — prompt, files, turns, codebase —
/// have to come from what the run captured, so this is where they are read from.
/// Assertions are the exception, resolved against the live tree by
/// [`crate::pipeline::resolve_grading_instrument`]: they are the measuring
/// instrument, not the treatment. Live-source detection is the other exception —
/// it needs the live path precisely because that is what it is looking for.
fn graded_skill_subdir(ctx: &RunContext, iteration_dir: &Path) -> PathBuf {
    let copied = iteration_dir.join(".skills").join(&ctx.skill_name);
    if copied.is_dir() {
        copied
    } else {
        ctx.skill_subdir.clone()
    }
}

/// The line that keeps a grading summary from being ambiguous about which
/// `evals.json` produced it. `Judge tasks: 0` reads as "my assertions did not
/// match" unless the file measured against is named beside it.
fn assertion_source_summary(instrument: &pipeline::GradingInstrument) -> String {
    let path = &instrument.source.path;
    if !instrument.source.refreshed {
        return format!("Assertions: {path} (unchanged since the run)");
    }
    let ids: Vec<&str> = instrument.refreshed_eval_ids().collect();
    format!(
        "Assertions: {path}\n  refreshed — differs from the run-time copy for {} eval(s): {}",
        ids.len(),
        ids.join(", ")
    )
}

/// Grade run records. Default mode emits LLM judge tasks (+ the skill-invocation
/// meta-check); `--finalize` folds judge responses into `grading.json`.
pub(crate) fn run_grade(args: GradeArgs) -> anyhow::Result<()> {
    let common = args.common;
    let ctx = run_context_from(&common)?;
    let iteration = resolve_iteration(&ctx, common.iteration)?;
    let dir = iteration_dir(&ctx, Some(iteration))?;

    let conditions_path = dir.join("conditions.json");
    if !conditions_path.exists() {
        bail!("missing: {}", conditions_path.display());
    }
    let conditions: crate::core::ConditionsRecord =
        serde_json::from_str(&std::fs::read_to_string(&conditions_path)?)?;

    // The treatment comes from the copy the run froze; the assertions come from
    // the live file. The documented workflow authors assertions from the run's
    // own paired evidence, after the dispatch they grade, so the frozen copy
    // does not hold them yet (#295).
    let skill_subdir = graded_skill_subdir(&ctx, &dir);
    let instrument = pipeline::resolve_grading_instrument(&skill_subdir, &ctx.skill_subdir)?;
    for warning in &instrument.warnings {
        eprintln!("⚠ {warning}");
    }
    println!("{}", assertion_source_summary(&instrument));

    let gctx = pipeline::GradeContext {
        iteration_dir: &dir,
        conditions: &conditions,
        evals: &instrument.evals,
        assertion_source: &instrument.source,
    };

    if args.finalize {
        let s = pipeline::finalize(&gctx)?;
        for w in &s.warnings {
            eprintln!("⚠ {w}");
        }
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
        let target_args = command_target_args(&ctx);
        println!("\nNext: eval-magic aggregate{target_args} --iteration {iteration}");
    } else {
        let diffs = pipeline::measure_iteration_diff_scopes(&dir)?;
        for w in &diffs.warnings {
            eprintln!("⚠ {w}");
        }
        println!(
            "Diff scope: {} measured, {} reused, {} missing baseline, {} shared environment",
            diffs.measured, diffs.reused, diffs.missing_baseline, diffs.shared_environment
        );
        let commands = pipeline::grade_command_checks(&dir, &instrument, common.overwrite)?;
        if commands.executed + commands.reused + commands.skipped_incomplete > 0 {
            println!(
                "Command checks: {} executed, {} reused, {} failed, {} skipped (missing run.json)",
                commands.executed, commands.reused, commands.failed, commands.skipped_incomplete
            );
        }
        for w in &commands.warnings {
            eprintln!("⚠ {w}");
        }
        let s = pipeline::emit_judge_tasks(&gctx)?;
        for w in &s.warnings {
            eprintln!("⚠ {w}");
        }
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
        let target_args = command_target_args(&ctx);
        let judge_guidance = judge_dispatch_guidance(&ctx, iteration);
        println!(
            "\nNext: {judge_guidance}\nThen run: eval-magic grade{target_args} --iteration {iteration} --finalize"
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
    for w in &benchmark.warnings {
        eprintln!("⚠ {w}");
    }
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
