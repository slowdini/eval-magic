# Product Context

## Why eval-magic exists

Agent skill changes usually ship unmeasured. Eval-magic turns "does this skill help?" into a
number: for each test case it dispatches a fresh subagent twice — with skill vs. without
(Mode A), or old skill vs. new (Mode B) — grades both against assertions, and reports the
pass-rate delta plus what it costs (time, tokens). A negative or zero delta is a signal to
revert. For *when and why* to write an eval at all, the methodology lives in the `slow-powers`
plugin's `evaluating-skills` skill.

## Problems it solves

- **Unmeasured skill authoring** — no way to know whether a skill shifts behavior or whether a
  wording change helps or hurts.
- **Harness lock-in** — run records follow JSON Schemas, so they grade identically no matter
  which harness authored them; new harnesses can be wired without changing the grading core.
- **Leaky comparisons** — live/installed skill sources would contaminate the without-skill
  condition, so the runner stages private copies per (eval, condition, run) and verifies
  isolation (`eval-magic docs isolation`).

## How it should work

- The loop is agent-drivable end to end: from inside an agent session, "Install eval-magic and
  help me run an eval on my-skill" is enough. `eval-magic run` stages everything, writes
  `RUNBOOK.md`, and prints a handoff; an agent reads and follows the runbook
  (dispatch → ingest → dispatch judges → finalize → read `benchmark.json`).
- Cost and confirmation: dispatching spends real tokens. The runner stages but does not
  dispatch; the write guard arms automatically and must be explicitly torn down before editing
  source again.

## User experience goals

- Discovery through the binary itself: `--help` is the primary surface; `eval-magic docs <topic>`
  ships reference topics that are version-matched to the install and readable offline.
- Documentation placement is governed by `docs/README.md`: anything a user of the installed
  binary could need ships in the binary (tiers 1–2); everything else stays internal (tier 3).
- One command to bring your own harness: `eval-magic harness init` scaffolds a descriptor
  (`eval-magic docs byoh`).
