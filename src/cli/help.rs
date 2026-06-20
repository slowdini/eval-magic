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
  # run builds the isolated env/ + RUNBOOK.md, then prints a handoff:
  #   cd into env/, start a fresh session, say \"Read and follow RUNBOOK.md\".
  # The fresh session walks the whole loop below from inside env/:
  #   …dispatch each task in dispatch.json as a fresh subagent…
  #   eval-magic ingest      # auto-resolves --subagents-dir from CLAUDE_CODE_SESSION_ID
  #                          # (override: --session-id <id> or --subagents-dir <path>)
  #   …dispatch each judge task ingest listed…
  #   eval-magic finalize
  #   eval-magic teardown
  eval-magic promote-baseline   # optional, from the prep session once benchmark.json lands

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

  # Codex harness: dispatch with stdin detached; ingest reads each task's codex-events.jsonl
  eval-magic run --harness codex
  eval-magic ingest --harness codex

  # Codex model selection: agent dispatches use --agent-model; judge tasks
  # use --judge-model unless an individual llm_judge assertion sets model.
  eval-magic run --harness codex --agent-model gpt-5-mini --judge-model gpt-5

  # OpenCode harness: stages under `.opencode/skills/`
  eval-magic run --harness opencode
  # ...dispatch each task with `opencode run`, then assemble records manually
  # until OpenCode transcript ingest is wired.
";
