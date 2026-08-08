# Project Brief

eval-magic is a one-stop CLI for running skill evals — structured measurements of whether an
agent skill actually shifts behavior. Version 0.7.0; Rust binary named `eval-magic`, released
from <https://github.com/slowdini/eval-magic>.

## Core requirements

- An eval dispatches a fresh subagent twice per test case — once with the skill loaded, once
  without (Mode A: new skill) or old version vs. new (Mode B: revision) — and grades both outputs
  against assertions. The pass-rate delta decides whether a skill or a change to it is worth
  shipping.
- Ships as a dependency-less prebuilt binary (macOS/Linux/Windows); Git is the only runtime
  prerequisite.
- Every artifact follows a documented JSON Schema (`schema/`, embedded in the binary), so records
  grade the same way regardless of which harness authored them.
- The runner builds the workspace, stages skills for discovery, generates dispatch prompts,
  assembles run records from transcripts, grades, and aggregates. It never dispatches subagents
  itself — the agent harness (Claude Code, Codex CLI, OpenCode, …) supplies dispatching.
- An agent or human must be able to use the installed tool with only what ships in the binary:
  the `--help` tree plus `eval-magic docs <topic>`.

## Source of truth for scope

- `README.md` — the complete operating guide: install, author cases, run the loop, read results,
  keep a baseline.
- `eval-magic --help` / `eval-magic docs <topic>` — flag-by-flag reference and shipped topics
  (`guide`, `byoh`, `isolation`).
- `AGENTS.md` — conventions for working on the codebase itself.

## Non-goals (for now)

- The runner never dispatches subagents; that stays with the harness (see BYOH:
  `eval-magic docs byoh`).
- No hosted docs site (deferred, issue #190) — GitHub rendering plus embedded topics cover it.
