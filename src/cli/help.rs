//! Long-form help text that is too large to inline as a doc-comment.
//!
//! clap derives short/long help from the `///` doc-comments in [`super::args`];
//! the worked examples below are attached to the top-level command via
//! `#[command(after_help = …)]`. Keeping the string here keeps `args.rs` focused
//! on the command tree.

/// Worked examples shown at the end of `skill-eval --help`.
pub(super) const AFTER_HELP: &str = "\
EXAMPLES:
  # Mode A — evaluate a new skill (with vs. without)
  skill-eval run --skill-dir <dir> --skill <name> --mode new-skill --guard
  # …dispatch each task in dispatch.json as a fresh subagent…
  skill-eval ingest --skill-dir <dir> --skill <name> --iteration 1 \\
    --subagents-dir ~/.claude/projects/<slug>/<session-id>/subagents/
  # …dispatch each judge task ingest listed…
  skill-eval finalize --skill-dir <dir> --skill <name> --iteration 1
  skill-eval promote-baseline --skill-dir <dir> --skill <name> --iteration 1   # optional
  skill-eval teardown --skill-dir <dir> --skill <name>

  # Mode B — evaluate a language change (edit-first)
  skill-eval snapshot --skill-dir <dir> --skill <name> --label baseline --ref HEAD
  skill-eval run --skill-dir <dir> --skill <name> --mode revision --baseline baseline --guard
  # …then the same ingest → finalize → teardown steps as Mode A.

  # Reduced-set / dry runs
  skill-eval run --skill-dir <dir> --skill <name> --mode new-skill --dry-run
  skill-eval run --skill-dir <dir> --skill <name> --mode new-skill --only case-a,case-b
  skill-eval run --skill-dir <dir> --skill <name> --mode new-skill --skip slow-case

  # Codex harness: dispatch with `codex exec --json`, then ingest without --subagents-dir
  skill-eval run --skill-dir <dir> --skill <name> --mode new-skill --harness codex
  skill-eval ingest --skill-dir <dir> --skill <name> --iteration 1 --harness codex
";
