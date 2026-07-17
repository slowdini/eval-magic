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
  eval-magic run
  # run builds per-(group, condition) envs + RUNBOOK.md (a human-followed recipe),
  # arming the write guard automatically when the harness supports it.
  # Follow it to dispatch each task in dispatch.json via `claude -p`, capturing each
  # task's outputs/claude-events.jsonl, then:
  #   eval-magic ingest      # reads each task's outputs/claude-events.jsonl
  #   …dispatch each judge task ingest listed…
  #   eval-magic finalize
  #   eval-magic teardown
  eval-magic promote-baseline   # optional, once benchmark.json lands

  # Mode B — evaluate a language change (edit-first)
  eval-magic snapshot --ref HEAD
  eval-magic run --mode revision
  # …then the same ingest → finalize → teardown steps as Mode A.

  # Reduced-set / dry runs
  eval-magic run --dry-run
  eval-magic run --only case-a,case-b
  eval-magic run --skip slow-case

  # Opt out of the auto-armed write guard
  eval-magic run --no-guard

  # Evaluate one skill from elsewhere, without staging sibling skills
  eval-magic run --skill ./skills/my-skill

  # Opt in to seeded environment parity: stage sibling skills from a skills dir
  eval-magic run --skill-dir ./skills --skill my-skill

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

  # Bring your own harness (docs/byoh.md): scaffold a commented descriptor +
  # notes skeleton into .eval-magic/harnesses/, fill in verified values, lint, run
  eval-magic harness init cool-custom-harness
  eval-magic harness lint .eval-magic/harnesses/cool-custom-harness.toml
  eval-magic harness list
  eval-magic run --harness cool-custom-harness
  # ...or load a one-off descriptor (its label becomes the default harness):
  eval-magic run --harness-file ./cool-custom-harness.toml

  # Inspect the resolved descriptor after layer merging (built-ins included)
  eval-magic harness show claude-code
";
