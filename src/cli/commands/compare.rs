//! Interactive paired-evidence report command.

use crate::cli::compare_args::CompareArgs;
use crate::cli::{iteration_dir, resolve_iteration, run_context_from};

pub(crate) fn run_compare(args: CompareArgs) -> anyhow::Result<()> {
    let ctx = run_context_from(&args.common)?;
    let iteration = resolve_iteration(&ctx, args.common.iteration)?;
    let dir = iteration_dir(&ctx, Some(iteration))?;
    let result = crate::pipeline::compare(&dir, iteration, &args.eval)?;

    println!("Wrote {}", result.path.display());
    Ok(())
}
