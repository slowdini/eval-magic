# OpenCode — harness implementation notes

> **Audience:** developers working on eval-magic's OpenCode support. Runtime usage lives in the
> README, `--help`, and the generated `RUNBOOK.md`; the enhancement model is in
> [progressive-enhancements.md](progressive-enhancements.md).

## Code map

Everything OpenCode-specific lives under `src/adapters/opencode/`:

| File | What's in it |
|------|--------------|
| `mod.rs` | `OpenCodeAdapter` — the trait impl, plus the slug/naming rules |
| `session.rs` | native `<available_skills>` XML block |

## What's wired

Native staging only: `--harness opencode` stages under `.opencode/skills/`, rewrites the staged
skill-under-test's frontmatter `name:` to a sanitized slug, and renders the `<available_skills>`
XML block in dispatch prompts. Everything else rides the trait's enhancement defaults:

- **No dispatch recipes** — `cli_next_steps` prints manual `opencode run` guidance instead of a
  copy-pasteable template.
- **No transcript ingest** — `cli_events_filename` is `None`, so the ingest pipeline never calls
  the (defaulted, erroring) parsers; `transcript_check` grades as unverifiable and the
  `__skill_invoked` meta-check uses the LLM-judge fallback.
- **No model flag** — `--agent-model` / `--judge-model` are recorded as provenance only.
- **No write guard** — `--guard` is rejected in the `run` preflight
  (`run_capabilities().supports_guard` is false); `detect-stray-writes` is the audit fallback.

## Naming rules

OpenCode skill names must be 1–64 characters, lowercase alphanumeric with single-hyphen separators
(no leading/trailing/consecutive hyphens), and match the containing directory name. `staged_slug`
sanitizes the generated slug while preserving the `slow-powers-eval-` cleanup prefix (truncating
the skill portion if the combination exceeds 64 chars); `validate_stage_name` applies the same
rules to `--stage-name` overrides. Sibling skills stage at their natural names and must already
satisfy the rules.

## Known inconsistency

The staged skill's frontmatter is rewritten to the slug (`rewrites_frontmatter_name` true) yet the
available-skills block advertises the *natural* name (`advertises_staged_slug_name` false) —
tracked for a separate fix.

## Wiring the next enhancements

- **Transcript ingest:** candidate sources are `opencode run --format json` and `opencode export`.
  Implement `parse_cli_events` / `parse_cli_events_full` in a new `transcript.rs` and set
  `cli_events_filename`; check whether the stream exposes a deterministic skill event before
  leaving `transcript_surfaces_skill_invocation` at its default.
- **Dispatch recipes:** an `opencode run` command template in a new `cli.rs`, wired through
  `cli_next_steps` / `cli_manifest_section` / `cli_judge_next_steps`.
- **Write guard:** needs an OpenCode pre-tool hook surface. Flip
  `run_capabilities().supports_guard` and `guard_armed_message` together — an invariant test in
  `src/adapters/harness.rs` enforces the lockstep.
