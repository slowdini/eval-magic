# OpenCode — harness implementation notes

> **Audience:** developers working on eval-magic's OpenCode support. Runtime usage lives in the
> README, `--help`, and the generated `RUNBOOK.md`; the enhancement model is in
> [progressive-enhancements.md](progressive-enhancements.md).

## Code map

The declarative half — including the `<available_skills>` XML block templates and the stage-name
rules (a regex + length cap) — is the descriptor file `harnesses/opencode.toml`;
`src/adapters/opencode/` keeps only the code capability the descriptor references:

| File | What's in it |
|------|--------------|
| `harnesses/opencode.toml` | the descriptor — every declarative value + the `opencode` slug reference |
| `mod.rs` | slug sanitization/truncation (the `opencode` slug capability) |

## What's wired

Native staging only: `--harness opencode` stages under `.opencode/skills/`, rewrites the staged
skill-under-test's frontmatter `name:` to a sanitized slug, and renders the `<available_skills>`
XML block in dispatch prompts, advertising the skill-under-test under its staged slug (matching
what OpenCode's skill tool lists, since it keys on the frontmatter name). Everything else rides
the trait's enhancement defaults:

- **No dispatch recipes** — `cli_next_steps` prints manual `opencode run` guidance instead of a
  copy-pasteable template.
- **No transcript ingest** — `cli_events_filename` is `None`, so the ingest pipeline never calls
  the (defaulted, erroring) parsers; `transcript_check` grades as unverifiable and the
  `__skill_invoked` meta-check uses the LLM-judge fallback.
- **No model flag** — `--agent-model` / `--judge-model` are recorded as provenance only.
- **No write guard** — auto-arm stays off (`run_capabilities().supports_guard` is false) and the
  `run` preflight warns naming the fallback; an explicit `--guard` warns and continues unguarded.
  `detect-stray-writes` is the audit fallback.

## Naming rules

OpenCode skill names must be 1–64 characters, lowercase alphanumeric with single-hyphen separators
(no leading/trailing/consecutive hyphens), and match the containing directory name. `staged_slug`
sanitizes the generated slug while preserving the `slow-powers-eval-` cleanup prefix (truncating
the skill portion if the combination exceeds 64 chars); `validate_stage_name` applies the same
rules to `--stage-name` overrides. Sibling skills stage at their natural names and must already
satisfy the rules.

## Wiring the next enhancements

- **Transcript ingest:** candidate sources are `opencode run --format json` and `opencode export`.
  Add a parser in a new `transcript.rs`, name it as a capability in
  `src/adapters/capabilities.rs`, and declare a `[transcript]` table (plus `[tools]` names) in
  `harnesses/opencode.toml`; check whether the stream exposes a deterministic skill event before
  leaving `surfaces_skill_invocation` at its default.
- **Dispatch recipes:** an `opencode run` command template in the descriptor's `[dispatch]` table
  (`exec_template` / `parallel_command_template` / `judge_command_template` +
  `next_steps_template` / `manifest_template`).
- **Write guard:** needs an OpenCode pre-tool hook surface — a new engine in
  `src/adapters/capabilities.rs`. Declare `[guard]` and `run.supports_guard` together — load-time
  descriptor validation enforces the lockstep.
