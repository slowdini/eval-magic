//! `run` and `dispatch-task` handlers.

use std::path::Path;

use crate::cli::args::{DispatchTaskArgs, RunArgs};
use crate::cli::run;
use crate::cli::{parse_id_list, run_context_with_bootstrap};

/// Build the iteration workspace and dispatch plan (the default action).
pub(crate) fn run_run(args: RunArgs) -> anyhow::Result<()> {
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
            guard: match (args.guard, args.no_guard) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            },
            stage_name: args.stage_name.as_deref(),
            plan_mode: args.plan_mode,
            runs: args.runs,
            agent_model: args.agent_model.as_deref(),
            judge_model: args.judge_model.as_deref(),
            label: args.label.as_deref(),
        },
    )?;
    Ok(())
}

pub(crate) fn run_dispatch_task(args: DispatchTaskArgs) -> anyhow::Result<()> {
    run::conversation::command_dispatch_task(
        Path::new(&args.dispatch),
        args.task_index,
        args.overwrite,
    )
}
