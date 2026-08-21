//! Long-form help text that is too large to inline as a doc-comment.
//!
//! clap derives short/long help from the `///` doc-comments in [`super::args`];
//! the worked examples below are attached to the top-level command via
//! `#[command(after_help = …)]`. Keeping the string here keeps `args.rs` focused
//! on the command tree.

/// Worked examples shown at the end of `eval-magic --help`.
pub(super) const AFTER_HELP: &str = "\
REQUIREMENTS:
  eval-magic supports Linux and macOS. On Windows, install and run eval-magic
  inside WSL; native Windows is unsupported. Keep the repository, workspace,
  and harness commands inside the same WSL environment. Git and a POSIX shell
  are required. Set EVAL_MAGIC_SH to select a specific sh.

EXAMPLES:
  # Scaffold a first eval and prepare its isolated comparison environments
  eval-magic init
  eval-magic run
  # run prepares the workspace but does not dispatch; eval-magic dispatch does.
  # Read the generated RUNBOOK.md end to end and follow it through dispatch,
  # ingest, judges, finalize, and teardown.
  # Artifacts land outside the skill's own repository; run prints the path, and
  # every command it suggests carries --workspace-dir. Set EVAL_MAGIC_WORKSPACE_DIR
  # to move the default. See: eval-magic docs isolation

  # Evaluate a revision: edit first, snapshot committed content, then compare
  eval-magic snapshot --ref HEAD
  eval-magic run --mode revision

  # Reduce cost while iterating on the suite
  eval-magic run --only case-a,case-b

  # Run the task against a real project instead of fixture files. The codebase
  # is declared in evals.json, not on the command line, so it stays a reviewed
  # property of the eval set
  eval-magic docs codebase

  # Select a built-in harness; `run --help` documents models and environment options
  eval-magic run --harness codex

  # Bring your own harness: scaffold a descriptor, then follow the shipped guide
  eval-magic harness init cool-custom-harness
  eval-magic docs byoh
";
