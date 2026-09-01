//! Hide the framework's own staged files from the sourced codebase's tooling.

use crate::adapters::adapter_for;
use crate::core::RunContext;
use crate::workspace::{IgnorePlan, apply_framework_ignore_entries};

use super::super::super::RunError;
use super::super::Resolved;
use super::super::envs::EnvTarget;

/// Teach each task environment's own ignore files to skip the paths the runner
/// placed, and report what was written.
///
/// Applied to *every* environment — both arms, every repetition, `--no-stage`
/// and `--dry-run` alike. A codebase whose lint or format step globs the tree
/// would otherwise report the staged skills as project failures, and only in
/// the arm that has them; an entry written in one arm alone would trade that
/// asymmetry for another.
pub(super) fn hide_framework_files(
    ctx: &RunContext,
    r: &Resolved,
    targets: &[EnvTarget],
) -> Result<(Vec<String>, Vec<String>), RunError> {
    let mut written: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let framework_paths = adapter_for(ctx.harness).framework_ignore_paths();
    for target in targets {
        let codebase = r.codebase_for(&target.eval_ids)?;
        let outcome = apply_framework_ignore_entries(
            &target.root,
            &IgnorePlan {
                declared: codebase.declared.ignore_files(),
                framework_paths: &framework_paths,
            },
        )?;
        written.extend(outcome.written);
        warnings.extend(outcome.warnings);
    }
    // Every environment holds the same codebase shape, so an undeduplicated
    // report would repeat itself once per run cell.
    for list in [&mut written, &mut warnings] {
        list.sort();
        list.dedup();
    }
    Ok((written, warnings))
}
