//! Workspace lifecycle command handlers: `snapshot`, `promote-baseline`, and the
//! end-of-run `teardown`.

use std::path::Path;

use anyhow::anyhow;

use crate::cli::args::{CommonArgs, PromoteBaselineArgs, SnapshotArgs};
use crate::cli::run;
use crate::cli::run_context_from;
use crate::sandbox;
use crate::workspace;

/// Snapshot the skill (`SKILL.md` + sibling assets, excluding `evals/`) into
/// `<workspace>/<skill>/snapshots/<label>/`, from the working tree or — with
/// `--ref` — a git ref. Ports eval-runner's `commandSnapshot`.
pub(crate) fn run_snapshot(args: SnapshotArgs) -> anyhow::Result<()> {
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
pub(crate) fn run_promote_baseline(args: PromoteBaselineArgs) -> anyhow::Result<()> {
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
pub(crate) fn run_teardown(args: CommonArgs) -> anyhow::Result<()> {
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
