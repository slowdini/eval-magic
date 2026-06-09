//! `run` — build the iteration workspace and dispatch plan (the default action).

use crate::cli::args::RunArgs;
use crate::cli::run;
use crate::cli::{parse_id_list, run_context_with_bootstrap};

/// Build the iteration workspace and dispatch plan (the default action). Ports
/// eval-runner's `commandRun`.
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
            guard: args.guard,
            stage_name: args.stage_name.as_deref(),
            plan_mode: args.plan_mode,
        },
    )?;
    Ok(())
}
