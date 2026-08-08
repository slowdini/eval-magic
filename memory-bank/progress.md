# Progress

## What works

- **Full harness support:** Claude Code and Codex CLI — staging, transcript ingest, write guard,
  shadow verification.
- **OpenCode:** native staging and transcript-ingest support (enhancement matrix in README
  "Harnesses").
- **BYOH:** `eval-magic harness init` scaffold, descriptor lint/list/show, data-only upstreaming
  PRs (`.github/PULL_REQUEST_TEMPLATE/harness-descriptor.md`).
- **Pipeline:** `run` (stages + RUNBOOK handoff) → dispatch → `ingest` (records, stray-write
  detection, grading) → judge dispatch → `finalize` → `benchmark.json`; workspaces with
  snapshot/promote/teardown; version-controlled baselines; per-assertion rollups; statistical
  floor (minimum attainable Fisher p-value); guard/permission denial accounting.
- **Docs:** `--help` tree, embedded topics `guide`/`byoh`/`isolation`, placement policy with
  drift guard.

## What's left / next

- **Cline harness support** — the next milestone (see `activeContext.md`). Scope it against the
  baseline-vs-enhancement contract in `docs/progressive-enhancements.md`.
- Hosted documentation — deferred until the criteria in `docs/README.md` are met (issue #190).

## Known issues

Tracked in GitHub issues: <https://github.com/slowdini/eval-magic/issues>. Don't duplicate them
here — link the issue number when a known issue matters to current work.

## Decision evolution

- The harness enhancement contract (`docs/progressive-enhancements.md`) defines the
  baseline-vs-enhancement ladder; each enhancement unlocks features with documented fallbacks.
- Progression visible in recent history: shadow verification of what dispatches actually loaded
  (#207), isolation topic (#216), judge robustness for incomplete batches (#215), heredoc guard
  fix (#214), assertion rollups (#220), Fisher floor (#221), rooted fixture sources (#222).
