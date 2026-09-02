//! Workspace lifecycle command handlers: `snapshot`, `promote-baseline`, and the
//! end-of-run `teardown`.

use std::collections::HashSet;
use std::fs;

use crate::adapters::adapter_for;
use crate::cli::args::{CommonArgs, PromoteBaselineArgs, SnapshotArgs};
use crate::cli::{
    command_target_args, iteration_dir, resolve_iteration, run_context_from, staged_env_roots,
};
use crate::core::SkillNames;
use crate::core::fs::artifact_path;
use crate::sandbox;
use crate::workspace;

/// Snapshot the skill (`SKILL.md` + sibling assets, excluding `evals/`) into
/// `<workspace>/<skill>/snapshots/<label>/`, from the working tree or — with
/// `--ref` — a git ref.
pub(crate) fn run_snapshot(args: SnapshotArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args.common)?;
    let label = args.label.unwrap_or_else(|| "baseline".to_string());
    let reference = args.reference.as_deref();

    let evals_path = ctx.skill_subdir.join("evals/evals.json");
    let (skill_names, multi_skill) =
        if evals_path.exists() {
            let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&evals_path)?)?;
            let authored: SkillNames =
                serde_json::from_value(value.get("skill_name").cloned().ok_or_else(|| {
                    anyhow::anyhow!("{}: missing skill_name", evals_path.display())
                })?)?;
            match authored {
                SkillNames::Many(names) => {
                    let mut unique = HashSet::new();
                    if names.is_empty() || names.iter().any(|name| !unique.insert(name)) {
                        anyhow::bail!("skill_name list must be non-empty and contain unique names");
                    }
                    if !names.contains(&ctx.skill_name) {
                        anyhow::bail!(
                            "eval owner '{}' must be listed in skill_name",
                            ctx.skill_name
                        );
                    }
                    (names, true)
                }
                SkillNames::One(_) => (vec![ctx.skill_name.clone()], false),
            }
        } else {
            (vec![ctx.skill_name.clone()], false)
        };

    let dest = if multi_skill {
        workspace::snapshot_set(
            &ctx.workspace_root,
            &ctx.skill_name,
            &ctx.skill_dir,
            &skill_names,
            &label,
            reference,
        )?
    } else {
        workspace::snapshot(
            &ctx.workspace_root,
            &ctx.skill_name,
            &ctx.skill_subdir,
            &label,
            reference,
        )?
    };

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

/// Promote an iteration's benchmark, gradings, and bounded judge evidence into
/// the skill's committed `evals/baseline/`, dropping a `.promoted.json` marker.
pub(crate) fn run_promote_baseline(args: PromoteBaselineArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args.common)?;
    let iteration = resolve_iteration(&ctx, args.common.iteration)?;

    let result = workspace::promote_baseline(&workspace::PromoteOptions {
        workspace_root: &ctx.workspace_root,
        skill_name: &ctx.skill_name,
        skill_subdir: &ctx.skill_subdir,
        iteration,
        harness: ctx.harness,
        label: args.label.as_deref(),
        agent_model: args.agent_model.as_deref(),
        judge_model: args.judge_model.as_deref(),
        responder_model: args.responder_model.as_deref(),
    })?;

    let n = result.gradings_copied;
    let evidence = result.evidence_copied;
    println!(
        "Promoted baseline for {} → {} (benchmark.json + {n} grading file{} + {evidence} evidence bundle{} + BASELINE.md)",
        ctx.skill_name,
        result.baseline_dir.display(),
        if n == 1 { "" } else { "s" },
        if evidence == 1 { "" } else { "s" }
    );
    if result.missing_gradings > 0 {
        let m = result.missing_gradings;
        eprintln!(
            "⚠ {m} run cell{} missing grading.json — omitted from the baseline. \
             Run grade/aggregate to complete the iteration before promoting.",
            if m == 1 { "" } else { "s" }
        );
    }
    if result.missing_evidence > 0 {
        let m = result.missing_evidence;
        eprintln!(
            "⚠ {m} run cell{} missing judge-evidence.md — retained gradings have no bounded evidence bundle. Re-grade the iteration to create it before promoting again.",
            if m == 1 { "" } else { "s" }
        );
    }
    match result.notes {
        workspace::NotesStatus::StubWritten => {
            println!("+ NOTES.md stub — fill in observations for this iteration.");
        }
        workspace::NotesStatus::RetainedFromPrior => {
            eprintln!(
                "⚠ NOTES.md retained from prior baseline — update {} for this iteration.",
                result.baseline_dir.join("NOTES.md").display()
            );
        }
    }
    Ok(())
}

/// End-of-run teardown: disarm the write guard, remove the staged skill set (and
/// prune a `.claude` the runner emptied), then reclaim the workspace, preserving
/// any iteration with uncommitted results.
pub(crate) fn run_teardown(args: CommonArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args)?;
    // Disarm the guard at the invocation cwd, then walk each per-(group, condition)
    // env marker (the human runs teardown from the iteration dir) before
    // `cleanup_workspace` reclaims the tree. Best-effort: a missing iteration just
    // skips the walk; `teardown_guard` is a no-op without a marker.
    let mut torn = sandbox::teardown_guard(&std::env::current_dir()?);
    if let Ok(dir) = iteration_dir(&ctx, args.iteration) {
        for env in staged_env_roots(&dir) {
            torn |= sandbox::teardown_guard(&env);
            if adapter_for(ctx.harness).skills_dir(&env).is_some() {
                crate::cli::run::staging::cleanup_staged_skills(&env, ctx.harness)?;
            }
        }
    }
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
        let target_args = command_target_args(&ctx);
        let lines = ws
            .kept_iterations
            .iter()
            .map(|k| format!("     - {} ({})", k.iteration, k.reason))
            .collect::<Vec<_>>()
            .join("\n");
        eprintln!(
            "⚠ Kept {} workspace iteration(s) with results not yet committed:\n{lines}\n   Commit them, e.g.:\n     eval-magic promote-baseline{target_args} --iteration <N>\n   or delete {}/ manually to discard.",
            ws.kept_iterations.len(),
            artifact_path(&ctx.workspace_root.join(&ctx.skill_name))
        );
    }
    Ok(())
}
