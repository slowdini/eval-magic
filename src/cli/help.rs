//! Long-form help text that is too large to inline as a doc-comment.
//!
//! clap derives short/long help from the `///` doc-comments in [`super::args`];
//! the worked examples below are attached to the top-level command via
//! `#[command(after_help = …)]`. Keeping the string here keeps `args.rs` focused
//! on the command tree.

/// Worked examples shown at the end of `eval-magic --help`.
pub(super) const AFTER_HELP: &str = "\
EXAMPLES:
  # Scaffold a first evals/evals.json for a raw skill directory
  eval-magic init --skill-dir <dir> --skill <name>

  # Mode A — evaluate a new skill (with vs. without)
  eval-magic run --skill-dir <dir> --skill <name> --mode new-skill --guard
  # …dispatch each task in dispatch.json as a fresh subagent…
  eval-magic ingest --skill-dir <dir> --skill <name> --iteration 1 \\
    --subagents-dir ~/.claude/projects/<slug>/<session-id>/subagents/
  # …dispatch each judge task ingest listed…
  eval-magic finalize --skill-dir <dir> --skill <name> --iteration 1
  eval-magic promote-baseline --skill-dir <dir> --skill <name> --iteration 1   # optional
  eval-magic teardown --skill-dir <dir> --skill <name>

  # Mode B — evaluate a language change (edit-first)
  eval-magic snapshot --skill-dir <dir> --skill <name> --label baseline --ref HEAD
  eval-magic run --skill-dir <dir> --skill <name> --mode revision --baseline baseline --guard
  # …then the same ingest → finalize → teardown steps as Mode A.

  # Reduced-set / dry runs
  eval-magic run --skill-dir <dir> --skill <name> --mode new-skill --dry-run
  eval-magic run --skill-dir <dir> --skill <name> --mode new-skill --only case-a,case-b
  eval-magic run --skill-dir <dir> --skill <name> --mode new-skill --skip slow-case

  # Codex harness: dispatch with `codex exec --json`, then ingest without --subagents-dir
  eval-magic run --skill-dir <dir> --skill <name> --mode new-skill --harness codex
  eval-magic ingest --skill-dir <dir> --skill <name> --iteration 1 --harness codex
";
