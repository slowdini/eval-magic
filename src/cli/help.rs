//! Long-form help text that is too large to inline as a doc-comment.
//!
//! clap derives short/long help from the `///` doc-comments in [`super::args`];
//! the worked examples below are attached to the top-level command via
//! `#[command(after_help = …)]`. Keeping the string here keeps `args.rs` focused
//! on the command tree.

/// Worked examples shown at the end of `eval-magic --help`.
pub(super) const AFTER_HELP: &str = "\
REQUIREMENTS:
  Git, plus a POSIX shell with jq, xargs, tr, and wc. The dispatch and judge
  recipes in the generated RUNBOOK.md are POSIX command lines — on Windows,
  run them in Git Bash (Git for Windows) or WSL, and install jq separately.
  Set EVAL_MAGIC_SH to select a specific sh.

EXAMPLES:
  # Scaffold a first eval and prepare its isolated comparison environments
  eval-magic init
  eval-magic run
  # run prepares the workspace but does not dispatch. Read the generated
  # RUNBOOK.md end to end and follow it through ingest, judges, finalize, and teardown.

  # Evaluate a revision: edit first, snapshot committed content, then compare
  eval-magic snapshot --ref HEAD
  eval-magic run --mode revision

  # Reduce cost while iterating on the suite
  eval-magic run --only case-a,case-b

  # Select a built-in harness; `run --help` documents models and environment options
  eval-magic run --harness codex

  # Bring your own harness: scaffold a descriptor, then follow the shipped guide
  eval-magic harness init cool-custom-harness
  eval-magic docs byoh
";
