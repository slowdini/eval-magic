//! `run` and `dispatch` handlers.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail};

use crate::adapters::adapter_for;
use crate::cli::args::{DispatchArgs, RunArgs};
use crate::cli::run;
use crate::cli::{iteration_dir, parse_id_list, run_context_from, run_context_with_bootstrap};
use crate::core::validate_agent_environment_entry;

fn parse_agent_environment(values: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
    let mut environment = BTreeMap::new();
    for assignment in values {
        let (name, value) = assignment
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid --agent-env {assignment:?}: expected KEY=VALUE"))?;
        validate_agent_environment_entry(name, value)
            .map_err(|message| anyhow!("invalid --agent-env {assignment:?}: {message}"))?;
        environment.insert(name.to_string(), value.to_string());
    }
    Ok(environment)
}

/// Build the iteration workspace and dispatch plan (the default action).
pub(crate) fn run_run(args: RunArgs) -> anyhow::Result<()> {
    let cli_agent_env = parse_agent_environment(&args.agent_env)?;
    let ctx = run_context_with_bootstrap(&args.common, args.bootstrap.clone())?;
    let mut agent_env = adapter_for(ctx.harness).dispatch_environment();
    agent_env.extend(cli_agent_env);
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
            guard: match (args.guard, args.no_guard) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            },
            stage_name: args.stage_name.as_deref(),
            plan_mode: args.plan_mode,
            runs: args.runs,
            agent_model: args.agent_model.as_deref(),
            agent_env,
            judge_model: args.judge_model.as_deref(),
            label: args.label.as_deref(),
        },
    )?;
    Ok(())
}

/// Execute a prepared iteration's tasks through the harness.
pub(crate) fn run_dispatch(args: DispatchArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args.common)?;
    let iteration_dir = iteration_dir(&ctx, args.common.iteration)?;
    // `--timeout 0` means "no deadline", which is the only way to say it with a
    // plain seconds flag.
    let timeout = (args.timeout > 0).then(|| std::time::Duration::from_secs(args.timeout));
    if args.judges {
        return dispatch_judges(&iteration_dir, &args, timeout);
    }
    let dispatch_path = iteration_dir.join("dispatch.json");
    if !dispatch_path.is_file() {
        bail!(
            "{} not found — run `eval-magic run` to prepare the iteration first",
            dispatch_path.display()
        );
    }

    let summary = run::drive::command_dispatch(
        &dispatch_path,
        &args.task_index,
        args.common.overwrite,
        timeout,
        args.jobs as usize,
    )?;

    println!(
        "\nDispatched {} task(s): {}",
        summary.reports.len(),
        summary.tally()
    );
    for warning in summary.warnings() {
        eprintln!("⚠ {warning}");
    }
    if summary.unusable() > 0 {
        bail!(
            "{} task(s) produced no usable result; rerun `eval-magic dispatch` to retry the \
             failures (a timed-out task keeps its record — pass --overwrite to redo it)",
            summary.unusable()
        );
    }
    Ok(())
}

/// Dispatch the judge tasks `ingest` emitted, reporting verdict completeness.
fn dispatch_judges(
    iteration_dir: &std::path::Path,
    args: &DispatchArgs,
    timeout: Option<std::time::Duration>,
) -> anyhow::Result<()> {
    let summary = run::drive::judges::command_dispatch_judges(
        iteration_dir,
        args.common.overwrite,
        timeout,
        args.jobs as usize,
    )?;
    println!(
        "\nDispatched {} judge task(s), skipped {}: {}",
        summary.dispatched,
        summary.skipped,
        summary.verdict_line()
    );
    for failure in &summary.failures {
        eprintln!("⚠ {failure}");
    }
    if !summary.complete() {
        bail!(
            "{} — rerun `eval-magic dispatch --judges` to fill the gaps",
            summary.verdict_line()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_agent_environment;

    #[test]
    fn agent_environment_assignments_split_once_and_last_value_wins() {
        let parsed = parse_agent_environment(&[
            "TZ=UTC".to_string(),
            "EMPTY=".to_string(),
            "TOKEN=a=b".to_string(),
            "TZ=America/New_York".to_string(),
        ])
        .unwrap();

        assert_eq!(parsed["TZ"], "America/New_York");
        assert_eq!(parsed["EMPTY"], "");
        assert_eq!(parsed["TOKEN"], "a=b");
    }

    #[test]
    fn agent_environment_assignments_reject_missing_equals_and_unsafe_names() {
        for assignment in ["TZ", "9TZ=UTC", "GIT_DIR=/outside"] {
            let error = parse_agent_environment(&[assignment.to_string()])
                .unwrap_err()
                .to_string();
            assert!(error.contains(assignment), "{assignment}: {error}");
        }
    }
}
