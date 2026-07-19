# OpenCode — harness implementation notes

> **Audience:** developers working on eval-magic's OpenCode support. Runtime usage lives in the
> README, `--help`, and the generated `RUNBOOK.md`; the enhancement model is in
> [progressive-enhancements.md](progressive-enhancements.md).

## Code map

The declarative half — including the `<available_skills>` XML block templates and the stage-name
rules (a regex + length cap) — is the descriptor file `harnesses/opencode.toml`;
`src/adapters/opencode/` keeps only the code capabilities the descriptor references:

| File | What's in it |
|------|--------------|
| `harnesses/opencode.toml` | the descriptor — every declarative value + the `opencode` slug and `opencode-events` parser references |
| `mod.rs` | slug sanitization/truncation (the `opencode` slug capability) |
| `transcript.rs` | `opencode run --format json` event-stream parsing (the `opencode-events` transcript capability) |

## What's wired

- **Native staging:** `--harness opencode` stages under `.opencode/skills/`, rewrites the staged
  skill-under-test's frontmatter `name:` to a sanitized slug, and renders the `<available_skills>`
  XML block in dispatch prompts, advertising the skill-under-test under its staged slug (matching
  what OpenCode's skill tool lists, since it keys on the frontmatter name).
- **Transcript ingest:** `opencode run --format json` emits a JSONL envelope stream
  (`{type, timestamp (epoch ms), sessionID, ...}`). The `opencode-events` parser normalizes
  `tool_use` parts (name at `part.tool`, args at `part.state.input`, outcome at
  `part.state.output`/`part.state.error`), sums `step_finish` `part.tokens` (cache reads excluded,
  matching codex accounting), takes the final message from the last `text` part, and measures
  duration from the envelope timestamps. The declarative extract tier was not sufficient here:
  tool args nest under `state.input` and timestamps are numbers, not RFC 3339 strings. The
  native `skill` tool (input `{name}`) is a deterministic skill-invocation event, so
  `surfaces_skill_invocation = true` with `skill_tool = "skill"` / `skill_arg = "name"` and the
  `__skill_invoked` meta-check grades from the transcript. `[tools]` declares the verified tool
  ids (`bash` = the shell tool id; `edit`/`write` take `filePath`; `apply_patch` takes
  `patchText` — the shared boundary policy reads all three spellings).
- **Dispatch recipes + model flag:** the `[dispatch]` templates run `opencode run --dir
  <eval-root> --format json --auto` (plus `[model] flag = "-m"`; they landed together because
  descriptor validation ties the judge template's `$model_arg` to a declared model flag).
  Verified against opencode v1.18.3 (`opencode run --help` and
  `packages/opencode/src/cli/cmd/run.ts`):
  - `--dir <dir>` resolves the project instance against `<dir>` and `chdir`s there, so the
    staged `.opencode/skills` in the env are discovered.
  - Piped stdin is **appended to the message** (`Bun.stdin.text()` when stdin is not a TTY), so
    every recipe detaches with `</dev/null>`, same as codex.
  - Headless permission asks are auto-**rejected** unless `--auto` is passed; explicit `deny`
    rules are still enforced under `--auto` (relevant once the write guard lands, #155).
  - `--format json` streams raw JSON events (`tool_use` / `text` / `step_finish` / `error`) to
    stdout; there is no `--output-last-message`, so the final message comes from the `text`
    events via transcript ingest (the dispatch prompt still asks for `outputs/final-message.md`,
    which wins when the agent writes it).
  - `-m` takes models in `provider/model` format; the value passes through verbatim.
  - Live-verified on v1.18.3 (one `opencode run` per the recipe + `ingest`, #153): the staged
    skill is discovered and invoked under its slug via the `skill` tool, and the events file
    ingests to a full run record. An operator config with an explicit `edit: deny` rule blocks
    the `final-message.md` write even under `--auto` (deny rules stay enforced) — record-runs
    then falls back to the transcript's last `text` part, so the run still records cleanly.

Everything else rides the trait's enhancement defaults:

- **No write guard** — auto-arm stays off (`run_capabilities().supports_guard` is false) and the
  `run` preflight warns naming the fallback; an explicit `--guard` warns and continues unguarded.
  `detect-stray-writes` is the audit fallback (tracked in
  [#155](https://github.com/slowdini/eval-magic/issues/155)).
- **No shadow preflight** — no automatic live-skill collision scan (tracked in
  [#154](https://github.com/slowdini/eval-magic/issues/154)); note OpenCode also discovers
  `.claude/skills` and `.agents/skills`, so skills installed for other harnesses can shadow.

## Naming rules

OpenCode skill names must be 1–64 characters, lowercase alphanumeric with single-hyphen separators
(no leading/trailing/consecutive hyphens), and match the containing directory name. `staged_slug`
sanitizes the generated slug while preserving the `slow-powers-eval-` cleanup prefix (truncating
the skill portion if the combination exceeds 64 chars); `validate_stage_name` applies the same
rules to `--stage-name` overrides. Sibling skills stage at their natural names and must already
satisfy the rules.

## Wiring the next enhancements

- **Shadow preflight (#154):** a new `opencode-skills` capability scanning the six discovery
  roots (project + global `.opencode/skills`, `.claude/skills`, `.agents/skills`).
- **Write guard (#155):** an OpenCode pre-tool hook surface — a project plugin at
  `.opencode/plugins/` whose `tool.execute.before` hook forwards to `eval-magic guard-hook
  --harness opencode` (a new guard engine variant; the shared arbiter and verdict path are
  reused). Declare `[guard]` and `run.supports_guard` together — load-time descriptor validation
  enforces the lockstep.
