# Codex — harness implementation notes

> **Audience:** developers working on eval-magic's Codex support. Runtime usage lives in the
> README, `--help`, and the generated `RUNBOOK.md`; the enhancement model is in
> [progressive-enhancements.md](progressive-enhancements.md).

## Code map

The declarative half (label, dirs, phrases, command templates, banner) is the descriptor file
`harnesses/codex.toml`; `src/adapters/codex/` keeps only the code capabilities the descriptor
references:

| File | What's in it |
|------|--------------|
| `harnesses/codex.toml` | the descriptor — every declarative value + capability references |
| `transcript.rs` | `item.completed` event-stream parsing (`codex-items`) |

The write guard has no per-harness code: the descriptor's `[guard]` block (hook file, matcher,
hook-entry and `{"decision": "block"}` verdict templates) is rendered by the generic engine in
`src/adapters/guard.rs`; the hidden `guard-codex` subcommand is its frozen hook entry-point
alias.

## Dispatch (`codex exec`)

- The approval policy must come **before** `exec`: `codex --ask-for-approval never exec …`
  (codex-cli rejects it after `exec`).
- `--cd <eval-root>` sets the dispatch cwd (no shell `cd` needed, unlike Claude Code).
- `--sandbox workspace-write` bounds writes to the env.
- `--json` streams events to stdout — captured as `outputs/codex-events.jsonl`; stderr goes to
  `codex-stderr.log` so progress/status text (e.g. stdin notices) stays out of the JSONL.
- `--output-last-message <outputs_dir>/final-message.md` writes the final-message file the
  pipeline reads.
- `</dev/null` matters when dispatching in parallel from a pipe (e.g. `xargs -P`): without it,
  Codex treats piped stdin as additional prompt context.

## Model flag

`-m <model>` is inserted before `--json` when `run --agent-model` is set. Judge recipes read each
task's resolved `model` from `judge-tasks.json` (`run --judge-model` sets the default; a
per-assertion `llm_judge.model` overrides it) and pass `-m "$model"` only when one is present.

## Staging & discovery

Skills stage under repo-local `.agents/skills/`. Codex keys discovery on the frontmatter `name:`,
so the staged skill-under-test's frontmatter is rewritten to the eval slug
(`rewrites_frontmatter_name` true) and the available-skills block advertises the slug
(`advertises_staged_slug_name` true).

## Transcript (`item.completed`)

`item.completed` events whose item type is not an agent message / reasoning / plan update become
tool invocations: `command_execution`, `file_change`, `web_search`, and MCP items.
`transcript_check` matches these parsed items. The JSONL exposes **no deterministic skill-tool
event**, so `transcript_surfaces_skill_invocation()` is false and the `__skill_invoked` meta-check
uses the LLM-judge fallback. Token accounting excludes cached input tokens.

## Write guard

`--guard` merges a `PreToolUse` hook into `.codex/hooks.json` (matcher:
`^Bash$|^apply_patch$|^Edit$|^Write$`, with a 30s timeout and status message). Dispatches must pass
`--dangerously-bypass-hook-trust` so the vetted project-local hook actually runs — the generated
recipes add it whenever the run was armed. The hook invokes the hidden `guard-codex` subcommand
(**stable on-disk contract — never rename**), which blocks via Codex's
`{ "decision": "block", "reason": "..." }` stdout shape and stays silent to allow. Teardown prunes
`.codex/` when restoring the original config leaves it empty (`guard_hook_cleanup_dir`).

## Running inside Codex itself

`eval-magic run --harness codex` from a Codex session writes `.agents/skills` (and, with
`--guard`, `.codex/hooks.json`). Those project-local Codex config paths are protected by Codex's
default workspace-write sandbox, so the runner may need approval/escalation or an external
terminal invocation. That approval is Codex's own permission boundary, not something eval-magic
bypasses.
