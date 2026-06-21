//! The post-dispatch / post-judge pipeline command handlers: the `ingest` and
//! `finalize` chains and each individual stage (`record-runs`,
//! `fill-transcripts`, `detect-stray-writes`, `grade`, `aggregate`).

use anyhow::bail;

use crate::adapters::{CliJudgeContext, adapter_for};
use crate::cli::args::{CommonArgs, GradeArgs, ResetBatchArgs, SwitchConditionArgs};
use crate::cli::command_target_args;
use crate::cli::run;
use crate::cli::{
    iteration_dir, resolve_iteration, resolve_subagents_dir, run_context_from, staged_env_roots,
};
use crate::core::{DispatchMechanism, RunContext};
use crate::pipeline;
use crate::sandbox;
use crate::validation;

const JUDGE_WORKER_PROMPT: &str = "Read the file at <dispatch_prompt_path> and follow it exactly. You are a judge worker only: write the JSON verdict to <response_path>, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers.";

fn judge_dispatch_guidance(ctx: &RunContext) -> String {
    match ctx.run_mode.mechanism() {
        DispatchMechanism::InSession => {
            format!("Dispatch each task as a judge subagent with:\n  {JUDGE_WORKER_PROMPT}")
        }
        DispatchMechanism::Cli => adapter_for(ctx.harness)
            .cli_judge_next_steps(CliJudgeContext {
                guard: sandbox::guard_is_armed(&ctx.stage_root),
            })
            .unwrap_or_else(|| {
                format!(
                    "Dispatch each task from judge-tasks.json with:\n  {JUDGE_WORKER_PROMPT}\nModel selection is recorded in judge-tasks.json, but this harness adapter has no judge CLI recipe wired yet."
                )
            }),
    }
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
        harness: Some(step.harness),
        run_mode: Some(step.run_mode),
        workspace_dir: step.workspace_dir.clone(),
        // The chain carries the already-resolved absolute subagents dir, so the
        // session id is no longer needed downstream.
        subagents_dir: step.subagents_dir.clone(),
        session_id: None,
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
    let iteration = resolve_iteration(&ctx, args.iteration)?;
    let resolved = resolve_subagents_dir(
        ctx.run_mode.mechanism(),
        args.subagents_dir.as_deref(),
        args.session_id.as_deref(),
    )?;
    let resolved = resolved.as_ref().map(|p| p.to_string_lossy().into_owned());

    let steps = run::steps::build_ingest_commands(&run::steps::StepParams {
        skill_dir: args.skill_dir.as_deref(),
        skill: args.skill.as_deref(),
        iteration,
        harness: ctx.harness,
        run_mode: ctx.run_mode,
        subagents_dir: resolved.as_deref(),
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
    let target_args = command_target_args(&ctx);
    let judge_guidance = judge_dispatch_guidance(&ctx);
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
        run_mode: ctx.run_mode,
        subagents_dir: None,
        workspace_dir: args.workspace_dir.as_deref(),
    });
    if let Some(failed) = run::steps::run_steps(&steps, run_step) {
        bail!("finalize stopped at '{failed}'. Fix the failure and re-run finalize.");
    }
    let target_args = command_target_args(&ctx);
    println!(
        "\n✅ Finalize complete. Read the benchmark above, then tear down: eval-magic teardown{target_args}"
    );
    // Warn if a guard is still armed. The cwd check covers the in-session flow (run
    // from inside `env/`); under Cli there is one env per (group, condition), so also
    // walk each per-env marker. `teardown` (not the cwd-only `teardown-guard`) is what
    // disarms them all.
    let mut armed = sandbox::guard_is_armed(&ctx.stage_root);
    if !armed
        && ctx.run_mode.mechanism() == DispatchMechanism::Cli
        && let Ok(dir) = iteration_dir(&ctx, Some(iteration))
    {
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

/// Switch the active condition batch in a single-session isolated run: remove the
/// *off-condition*'s staged skill from `env/.claude/skills/` so the next batch the
/// session dispatches cannot read it. `--condition` names the condition about to be
/// dispatched (the one to keep); its counterpart is removed. Idempotent, and a hard
/// barrier — the runbook instructs the operator to join every Task subagent of the
/// prior batch first. Resolves the iteration from `--workspace-dir`, so it runs from
/// `cwd = env/`. The guard marker is a sibling file of the slug subtree, so removing
/// the slug dir leaves it (and an armed guard) intact.
pub(crate) fn run_switch_condition(args: SwitchConditionArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args.common)?;
    let dir = iteration_dir(&ctx, args.common.iteration)?;

    let conditions_path = dir.join("conditions.json");
    if !conditions_path.exists() {
        bail!("missing: {}", conditions_path.display());
    }
    let conditions: crate::core::ConditionsRecord =
        serde_json::from_str(&std::fs::read_to_string(&conditions_path)?)?;

    // `--condition` names the arm to KEEP; its counterpart is the off-condition to
    // remove. Validate against the recorded conditions so a typo fails loudly
    // instead of silently no-opping.
    let names: Vec<&str> = conditions
        .conditions
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    if !names.contains(&args.condition.as_str()) {
        bail!(
            "unknown --condition '{}'; this iteration's conditions are: {}",
            args.condition,
            names.join(", ")
        );
    }
    let off = conditions
        .conditions
        .iter()
        .find(|c| c.name != args.condition)
        .ok_or_else(|| anyhow::anyhow!("no off-condition to switch away from"))?;

    let skills_dir = run::staging::skills_dir_for_harness(&dir.join("env"), ctx.harness);
    match off.staged_skill_slug.as_ref() {
        // The off-condition staged a skill: remove exactly its slug subtree. We do
        // NOT use `cleanup_staged_skills` (it prefix-scans and would remove both
        // arms' slugs and prune the dir) — only this one slug must go.
        Some(Some(slug)) => {
            let slug_dir = skills_dir.join(slug);
            if slug_dir.exists() {
                std::fs::remove_dir_all(&slug_dir)?;
                println!(
                    "Switched to '{}': removed off-condition '{}' staged skill ({}).",
                    args.condition,
                    off.name,
                    slug_dir.display()
                );
            } else {
                println!(
                    "Switched to '{}': off-condition '{}' staged skill already absent — nothing to do.",
                    args.condition, off.name
                );
            }
        }
        // The off-condition never staged a skill (e.g. the new-skill control arm),
        // so there is nothing to hide.
        _ => println!(
            "Switched to '{}': off-condition '{}' has no staged skill — nothing to remove.",
            args.condition, off.name
        ),
    }
    Ok(())
}

/// Swap the active isolation batch in a single-session (in-session) isolated run:
/// wipe the shared `env/` working tree — keeping the staged skills and the
/// `.eval-magic-outputs/` tree — and re-seed it with `--group`'s fixtures, so the
/// next batch starts from a clean tree the prior batch's fixtures and stray writes
/// can't taint. A hard barrier: the runbook joins every Task subagent of the prior
/// batch first. Resolves the iteration from `--workspace-dir`, so it runs from
/// `cwd = env/`.
pub(crate) fn run_reset_batch(args: ResetBatchArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args.common)?;
    let dir = iteration_dir(&ctx, args.common.iteration)?;
    let env_dir = dir.join("env");
    if !env_dir.exists() {
        bail!("missing env dir: {}", env_dir.display());
    }

    let dispatch_path = dir.join("dispatch.json");
    if !dispatch_path.exists() {
        bail!("missing: {}", dispatch_path.display());
    }
    let dispatch: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&dispatch_path)?)?;
    let tasks = dispatch["tasks"].as_array().cloned().unwrap_or_default();

    // Groups are tagged on tasks only when there is more than one. Validate against
    // them so a typo (or a needless reset on a single-group run) fails loudly.
    let group_ids: std::collections::BTreeSet<&str> =
        tasks.iter().filter_map(|t| t["group"].as_str()).collect();
    if !group_ids.contains(args.group.as_str()) {
        if group_ids.is_empty() {
            bail!(
                "unknown --group '{}'; this iteration has a single group, so reset-batch is not needed.",
                args.group
            );
        }
        bail!(
            "unknown --group '{}'; this iteration's groups are: {}",
            args.group,
            group_ids.into_iter().collect::<Vec<_>>().join(", ")
        );
    }

    // The group's declared, env-relative fixture dests (deduped across its tasks).
    let mut dests: Vec<String> = Vec::new();
    for t in &tasks {
        if t["group"].as_str() != Some(args.group.as_str()) {
            continue;
        }
        if let Some(fixtures) = t["fixtures"].as_array() {
            for f in fixtures.iter().filter_map(|f| f.as_str()) {
                if !dests.iter().any(|d| d == f) {
                    dests.push(f.to_string());
                }
            }
        }
    }

    // Full wipe: drop every entry in env/ except the staged skills, the outputs
    // tree, and the runbook — so a prior batch's fixtures and any stray writes can't
    // leak into this one.
    const KEEP: &[&str] = &[
        ".claude",
        ".agents",
        ".codex",
        ".opencode",
        ".eval-magic-outputs",
        "RUNBOOK.md",
    ];
    for entry in std::fs::read_dir(&env_dir)? {
        let entry = entry?;
        if KEEP.iter().any(|k| entry.file_name() == **k) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }

    // Re-seed this group's fixtures from the skill's evals/ dir.
    for dest in &dests {
        let src = ctx.skill_subdir.join("evals").join(dest);
        let dst = env_dir.join(dest);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        run::copy_entry(&src, &dst)?;
    }

    println!(
        "Reset to group '{}': wiped the env working tree and re-seeded {} fixture(s).",
        args.group,
        dests.len()
    );
    Ok(())
}

/// Assemble `run.json` + `timing.json` for every task in the iteration's
/// `dispatch.json`.
pub(crate) fn run_record_runs(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let mechanism = ctx.run_mode.mechanism();
    let resolved = resolve_subagents_dir(
        mechanism,
        args.subagents_dir.as_deref(),
        args.session_id.as_deref(),
    )?;
    let subagents_dir = resolved.as_deref();

    let dir = iteration_dir(&ctx, args.iteration)?;
    let result =
        pipeline::record_runs(&dir, ctx.harness, mechanism, subagents_dir, args.overwrite)?;

    println!(
        "\nRecorded: {}, skipped (existing run.json): {}, skipped (no final message): {}, missing transcript: {}",
        result.recorded,
        result.skipped_existing,
        result.skipped_no_final_message,
        result.missing_transcript
    );
    if let Some(warning) = result.transcript_warning(ctx.harness, mechanism) {
        eprintln!("{warning}");
    }
    Ok(())
}

/// Populate `tool_invocations` from persisted transcripts for every `run.json` in
/// the iteration.
pub(crate) fn run_fill_transcripts(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    let mechanism = ctx.run_mode.mechanism();
    let resolved = resolve_subagents_dir(
        mechanism,
        args.subagents_dir.as_deref(),
        args.session_id.as_deref(),
    )?;
    let subagents_dir = resolved.as_deref();

    let dir = iteration_dir(&ctx, args.iteration)?;
    let result =
        pipeline::fill_transcripts(&dir, ctx.harness, mechanism, subagents_dir, args.overwrite)?;

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
    let iteration = resolve_iteration(&ctx, args.iteration)?;
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
    let clean = t.violations == 0 && t.warnings == 0 && t.live_source_reads == 0;
    if clean && report.invocations_inspected == 0 {
        eprintln!(
            "⚠ Unverifiable — 0 transcript tool-calls inspected. Stray-write detection had nothing to check (every run's tool_invocations is empty); link transcripts first, then re-run (see the record-runs warning about passing agent_description verbatim / pointing --subagents-dir at the right session)."
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
        let target_args = command_target_args(&ctx);
        println!("\nNext: eval-magic aggregate{target_args} --iteration {iteration}");
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
        let target_args = command_target_args(&ctx);
        let judge_guidance = judge_dispatch_guidance(&ctx);
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
