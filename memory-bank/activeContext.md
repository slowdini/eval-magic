# Active Context

_Last updated: 2026-08-07 (memory bank initialized)._

## Current state

- Version 0.7.0; base branch `dev`, in sync with `origin/dev`; recent merges: #220 per-assertion
  benchmark rollups, #221 minimum attainable Fisher p-value, #222 rooted fixture sources.
- Claude Code and Codex CLI are fully wired harnesses; OpenCode has native staging and
  transcript-ingest support. See `progress.md` and the README "Harnesses" section for the
  per-harness enhancement matrix.

## Recent changes (this session)

- Initialized the Cline Memory Bank (`memory-bank/`) and its rule file
  (`.clinerules/memory-bank.md`). Files are concise pointers into the authoritative docs
  (README, AGENTS.md, docs/), not duplicates.

## Next steps

1. **Add eval-magic harness support for Cline** — the explicitly planned next milestone. Follow
   the harness baseline-vs-enhancement contract in `docs/progressive-enhancements.md`; study the
   per-harness notes (`docs/claude-notes.md`, `docs/codex-notes.md`, `docs/opencode-notes.md`)
   and the BYOH guide (`eval-magic docs byoh`) before designing the descriptor and adapter.
2. Keep the memory bank current as that work lands — especially this file after each session.

## Active decisions & considerations

- Documentation placement follows `docs/README.md`: anything Cline-harness users of the installed
  binary could need must ship in `--help` or an embedded `eval-magic docs` topic, never only in
  tier-3 dev docs.
- The runner must never dispatch subagents itself; Cline support, like other harnesses, provides
  dispatching through its own CLI surface.
- Isolation honesty: declare `[shadow] isolates_live_sources` only when verified
  (`eval-magic docs isolation`).

## Learnings & preferences

- Read `AGENTS.md` + README before implementing; the repo is strict about fmt/clippy gates and
  shipped-doc drift guards (`tests/cli/docs.rs`).
