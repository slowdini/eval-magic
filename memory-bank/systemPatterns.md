# System Patterns

## Module map (`src/`)

- `main.rs` — thin binary entry point; all behavior lives in the library crate.
- `lib.rs` — library root exposing the submodules below.
- `cli/` — clap command tree. `args.rs` carries the doc comments that drive `--help`; `help.rs`
  the worked examples. Commands: `run`, `init`, `validate`, `pipeline`, `harness` (+ `probe`),
  `workspace`, `guard`, `docs`. `cli/run/` holds dispatch, conversation, steps, and `staging/`.
- `core/` — shared primitives: capabilities, context, fs, runtime, types.
- `pipeline/` — the eval engine: `record_runs/` (run-record assembly), `aggregate/` (+ assertion
  rollups), `grade/` (`command_check`, `transcript_check`, `judge_tasks`, `finalize`,
  `diff_scope`), plus `detect_stray_writes`, `fill_transcripts`, `git_isolation`,
  `guard_denials`, `permission_denials`, `session_surface`, `shadow_verification`, `slots`.
- `adapters/` — per-harness adapters: `claude_code/`, `codex/`, `opencode/` (each with
  transcript parsing and shadow support), `descriptor/` (TOML descriptors, `layers`,
  `validation`), `guard`, `skill_shadow/` (artifact, resolution, verification), `registry`,
  `capabilities`, `extract`, `skills_block`.
- `sandbox/` — write guard: `policy`, `decide`, `shell_targets`, `guard`, `install`,
  `git_command`.
- `validation/` — `evals` file validation against the embedded JSON Schemas (`schema/`),
  batch handling.
- `workspace/` — snapshot, promote, teardown of eval workspaces.

## Bundled assets

- `harnesses/*.toml` — built-in harness descriptors (claude-code, codex, opencode, template),
  schema-validated and embedded; `opencode-guard-plugin.js` is the OpenCode write-guard plugin
  template; `template-notes.md` documents the template fields.
- `schema/` — 14 JSON Schemas for every artifact (evals, run-record, grading, benchmark,
  judge-tasks, guard-denials, permission-denials, stray-writes, session-surface,
  harness-descriptor, conversation, command-check, diff-scope, plugin-shadow).
- `profiles/shared/` — the shared plan-mode procedure profile (`--plan-mode`) and runbook
  templates, embedded in the binary.

## Key patterns

- **Thin binary, library-first.** New behavior goes in the library crate so it stays testable;
  `main.rs` stays trivial.
- **Warnings travel on result structs.** Library modules (`pipeline`, `workspace`, `sandbox`,
  `adapters`) never print. They return warning strings on their result struct —
  `#[serde(skip)]` when that struct is also a serialized artifact — and the `cli` handler prints
  them with the `⚠ ` prefix.
- **Declarative harness descriptors with layering.** `harnesses/*.toml` hold declarative values
  plus named-capability references; layer-merge rules let BYOH descriptors override
  (`eval-magic docs byoh`).
- **Isolation by staging, verified from transcripts.** Skills are staged into one private env
  per (eval, condition, run) under `.eval-magic/<skill>/iteration-N/`; shadow verification
  checks what dispatches actually loaded (`eval-magic docs isolation`).
- **Write guard.** Sandbox policy blocks writes outside the eval env; it arms automatically on
  `run` and requires explicit teardown before editing source.
- **Schema-first artifacts.** Every persisted artifact validates against an embedded schema;
  `tests/cli/docs.rs` drift-guards that every `eval-magic docs <topic>` mention in help output
  names a real topic.

## Testing patterns

- Unit tests: inline `#[cfg(test)] mod tests` at the bottom of the file they exercise — the
  default. Extract only when a module outgrows its file: themed submodules under `<module>/tests/`
  (`pipeline/record_runs/`, `cli/run/staging/`) or a single `<topic>_tests.rs` sibling
  (`adapters/guard/guard_denial_tests.rs`).
- Integration tests split by surface area: `tests/cli/` (command-line boundary, workspace
  artifacts) and `tests/run/` (end-to-end run behavior), with `tests/fixtures/` and
  `tests/golden/`.
