//! Long-form help text that is too large to inline as a doc-comment.
//!
//! clap derives short/long help from the `///` doc-comments in [`super::args`];
//! the worked examples below are attached to the top-level command via
//! `#[command(after_help = …)]`. Keeping the string here keeps `args.rs` focused
//! on the command tree.

/// Worked examples shown at the end of `eval-magic --help`.
pub(super) const AFTER_HELP: &str = "\
EXAMPLES:
  # Scaffold a first evals/evals.json from inside a skill directory
  eval-magic init

  # Mode A — evaluate a new skill (with vs. without)
  eval-magic run --guard
  # …dispatch each task in dispatch.json as a fresh subagent…
  eval-magic ingest \\
    --subagents-dir ~/.claude/projects/<slug>/<session-id>/subagents/
  # …dispatch each judge task ingest listed…
  eval-magic finalize
  eval-magic promote-baseline   # optional
  eval-magic teardown

  # Mode B — evaluate a language change (edit-first)
  eval-magic snapshot --ref HEAD
  eval-magic run --mode revision --guard
  # …then the same ingest → finalize → teardown steps as Mode A.

  # Reduced-set / dry runs
  eval-magic run --dry-run
  eval-magic run --only case-a,case-b
  eval-magic run --skip slow-case

  # Evaluate one skill from elsewhere, without staging sibling skills
  eval-magic run --skill ./skills/my-skill --guard

  # Opt in to seeded environment parity: stage sibling skills from a skills dir
  eval-magic run --skill-dir ./skills --skill my-skill --guard

  # Codex harness: dispatch with `codex exec --json`, then ingest without --subagents-dir
  eval-magic run --harness codex
  eval-magic ingest --harness codex
";
