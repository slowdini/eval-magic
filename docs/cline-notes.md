# Cline — harness implementation notes

> **Audience:** developers working on eval-magic's Cline support. Runtime usage lives in the
> README, `--help`, and the generated `RUNBOOK.md`; the enhancement model is in
> [progressive-enhancements.md](progressive-enhancements.md). The don't-guess rule
> ([byoh guide](guides/byoh.md)) requires every descriptor value to trace to Cline's own
> documentation or to observed output — that evidence is recorded here.

## Verified against

- Harness CLI + version: `cline 3.0.52` (`cline --version`), installed via npm (`npm i -g cline`)
- Date verified: 2026-08-11
- Documentation consulted: <https://docs.cline.bot/cline-cli/overview>,
  <https://docs.cline.bot/cline-cli/cli-reference>,
  <https://docs.cline.bot/customization/skills>,
  <https://docs.cline.bot/customization/plugins>, <https://docs.cline.bot/sdk/plugins>,
  <https://agentskills.io/specification>
- Observed output: a real `cline --json --auto-approve true` dispatch in a throwaway directory
  (676-line NDJSON capture exercising `read_files`, `run_commands`, and two `skills`
  invocations), `cline history --json`, several `--id` resume attempts, and
  `eval-magic harness lint harnesses/cline.toml --as-builtin --probe --yes` (the live probe
  rendered the exec template, dispatched it, and recovered a non-empty `final-message.md`).

## Code map

Cline support is **descriptor-only**: `harnesses/cline.toml` plus the mechanical
`EMBEDDED_DESCRIPTORS` entry in `src/adapters/descriptor.rs`. There is no
`src/adapters/cline/` module — the transcript summary rides the declarative
`[transcript.extract]` tier, and no guard, shadow preflight, slug, or parser capability is
declared. Named-capability gaps are tracked in "Wiring the next enhancements" below.

## Verification record

Every non-comment field in `harnesses/cline.toml`, with its source. "Probe capture" refers to
the observed dispatch described above.

| Descriptor field | Value | Source |
|------------------|-------|--------|
| `label` | `cline` | chosen name |
| `skills_dir` | `.cline/skills` | docs CLI reference ("Configuration Files": project `.cline/skills/`); confirmed by probe capture: a skill staged at `<cwd>/.cline/skills/kebab-probe-skill` was invoked via `--cwd` alone |
| `config_dirs` | `[".cline"]` | same docs section (project config root is `.cline/`: rules/skills/hooks/plugins/mcp.json) |
| `[tools] write` | `["editor"]` | Cline 3.x tool-routing table maps `write_to_file`/`replace_in_file`/`apply_diff` to the `editor` executor; the stream's `toolName` uses executor names (observed `read_files`, `run_commands`, `skills` in the probe capture) |
| `[tools] patch` | `[]` | no patch-shaped executor in the routing table (`apply_diff` routes to `editor`) |
| `[tools] shell` | `["run_commands"]` | routing table maps `execute_command`/`bash` to `run_commands`; probe capture shows `run_commands` with `input.commands: [...]` |
| `[tools] read` | `["read_files", "search_codebase"]` | routing table (`read_file`→`read_files`; `search_files`/`list_files`/`list_code_definition_names`→`search_codebase`/`run_commands`); `read_files` observed in the probe capture |
| `staging.slug_template` | `{prefix}{iteration}-{condition}__{skill_name}` | the default; works because Cline 3.0.52 does **not** enforce the Agent Skills naming spec — probe capture: a skill named `underscore_probe_skill` was discovered and invoked (`skills` call with `input.skill = "underscore_probe_skill"`) |
| `staging.stage_name_pattern` / `stage_name_invalid_message` | `^[a-z0-9][a-z0-9_-]*$` | eval-magic-authored constraint for `--stage-name` overrides only (keeps overrides slug-safe); not a Cline rule — Cline accepted underscores |
| `staging.rewrites_frontmatter_name` / `advertises_staged_slug_name` | `true` / `true` | docs skills page: frontmatter `name` "must exactly match the directory name"; staged dirs get the slug, so the frontmatter is rewritten and the slug advertised. Probe capture: the agent invoked skills by their frontmatter names |
| `staging.surface_phrase` / `unresolved_phrase` | "as a Cline skill" / "If it does not load as a Cline skill" | phrasing authored for eval-magic prompts (no Cline equivalent string) |
| `skills_block.header` / `item` | `## Skills` / `- {name}: {description} (file: {path})` | eval-magic-authored markdown block (same shape as the codex descriptor); Cline's own skill list rendering is internal |
| `transcript.events_filename` | `cline-events.jsonl` | exec template captures `--json` stdout to this file (probe capture) |
| `transcript.surfaces_skill_invocation` | `false` | the deterministic meta-check matches the skill arg at the top level of invocation args, but Cline nests it (`input.skill`) — see "What's wired" |
| `[transcript.extract.tools]` | `where type=agent_event, event.type=content_start, event.contentType=tool; item=event; name_field=toolName` | probe capture: one `content_start` per tool call, self-contained (`toolCallId`, `toolName`, `input`) |
| `[transcript.extract.final_text]` | `where type=run_result; field=text` | probe capture: terminal `run_result` line carries the full final text |
| `[transcript.extract.assistant_messages]` | `where type=agent_event, event.type=content_end, event.contentType=text; field=event.text` | probe capture: `content_end` text events carry complete (non-streaming) text blocks in order |
| `[transcript.extract.tokens]` | `sum usage.inputTokens+usage.outputTokens, subtract usage.cacheReadTokens` over `run_result` | probe capture: `run_result.usage` shape; cache-read subtraction matches codex accounting (`inputTokens` includes cached input) |
| `[transcript.extract.duration]` | `field=durationMs` over `run_result` | probe capture: `run_result.durationMs` (28233 for the probe run) |
| `model.flag` | `-m` | `cline --help`: `-m, --model <model-id>` |
| `dispatch.capture_prefix` | `cline` | chosen name (judge capture files `$response_base.cline-events.jsonl`) |
| `dispatch.exec_template` / `parallel_command_template` | see descriptor | flags from `cline --help` (`--act` from the 3.0.52 binary’s hidden option registration + behavioral write test); `--json` NDJSON stdout and `</dev/null` stdin detach from the docs CLI overview (piped stdin becomes prompt context); final-message jq recovery verified by the live probe |
| `dispatch.judge_command_template` | `cline --cwd "{cwd}" --json --auto-approve true $model_arg \` | same flag sources; render-checked by the live probe |
| `dispatch.next_steps_template` / `manifest_template` | see descriptor | prose authored for eval-magic artifacts (same structure as the other built-ins) |

## Dispatch quirks

- One-shot `cline "prompt"` runs a single turn and exits; headless mode activates on `--json`,
  piped stdin, or redirected stdout (docs CLI overview; confirmed by the probe).
- `-c/--cwd <dir>` sets the dispatch cwd (no shell `cd` needed, unlike Claude Code); staged
  `.cline/skills` are discovered from there (probe capture).
- `--act` pins act mode. It is a hidden flag (`.hideHelp()` in the CLI source, present in the
  3.0.52 binary as `-a, --act` "Run in act mode"), and it is load-bearing: the operator’s global
  `planActMode` setting (`~/.cline/data/settings/global-settings.json`) otherwise applies to
  headless dispatches — with `planActMode: plan` set, both smoke-eval arms ran read-only and
  stalled asking to "switch to act mode" instead of writing files (the same silent-degradation
  class as Claude Code’s `acceptEdits`). Verified behaviorally: an `--act` dispatch created and
  wrote `act-check.txt`.
- Tool calls auto-approve by default; recipes pin `--auto-approve true` against default drift.
  The docs warn that with approval required, non-TTY dispatches auto-DENY every call — the same
  silent-degradation trap as Claude Code's `acceptEdits`, so the flag stays explicit.
- Piped stdin is appended to the prompt context (docs CLI overview), so every recipe detaches
  with `</dev/null>`, same as codex.
- There is no `--output-last-message`: the terminal `run_result` NDJSON event carries the final
  text, and the exec template's trailing jq step writes `final-message.md` from the captured
  events (the harness probe's final-message contract checks that file; ingest recovers the text
  from the events file via `extract.final_text`).
- Stream shape (3.0.52): `agent_event` wrappers (`content_start` per streaming chunk for
  text/reasoning — 634 in the probe — but once per tool call; `content_end` with complete
  blocks; `usage`; `iteration_start`/`iteration_end`; `done`), plus `hook_event` lifecycle
  lines, plus one `run_result`. The docs CLI reference still documents say/ask lines — the
  binary capture is authoritative (Cline's own README jq example also filters
  `type == "agent_event"`).
- Timestamps are RFC 3339 strings (`ts`), and `run_result.durationMs` is a millisecond field —
  both duration rules were available; the field pick is exact.
- Session ids: `cline history --json` lists `sessionId` (e.g. `1786476589660_gcjjm`). The
  `hook_event` lines in the stream carry a *different* `taskId` (`conv_...`) that is not the
  resume id, and the resume `sessionId` appears nowhere in the `--json` stream (grepped), so
  `[transcript.extract.session_id]` has no source.
- `--id <session-id>` headless resume is broken in 3.0.52: `cline --id <sessionId> --json
  "prompt"` errors `JSON output mode requires a prompt argument or piped stdin` — reproduced
  with a positional prompt, with piped stdin, and with the exact `sessionId` from
  `cline history --json`. With no id in the stream and resume erroring, `[conversation]` is
  undeclarable; `run` rejects scripted `turns` evals on this harness until both are fixed
  upstream.
- `-t/--timeout` exists (default 0 = none) and is deliberately not pinned in recipes; the
  harness probe's `--probe-timeout` bounds the live check.

## What's wired

- **Native staging** under `.cline/skills/` with the default slug template (Cline 3.0.52
  accepts underscore names, so no slug capability), frontmatter rewrite to the slug, and the
  available-skills block in dispatch prompts.
- **Dispatch recipes + model flag**: single, parallel, and judge templates
  (`cline --cwd ... --act --json --auto-approve true`, `-m`); live-probe verified end to end.
  The parallel template’s jq step uses escaped double quotes because the block nests inside the
  shared scaffold’s `sh -c '...'` — single quotes terminate the body (caught by the smoke eval).
- **Declarative transcript ingest**: tool invocations (args nested under `input`), final text
  from `run_result.text`, ordered assistant messages from `content_end` text blocks, token
  totals from `run_result.usage` (cache reads subtracted), duration from `run_result.durationMs`.
  `transcript_check` works; patterns match the `"<name> <compact-json-args>"` rendering, so
  Cline args match at their nested shape (e.g. `run_commands.*"commands"`).
- **Riding documented fallbacks** (the `run` preflight names each): no guard (detect-stray-writes
  is the after-the-fact audit), no shadow preflight (warning only), `surfaces_skill_invocation =
  false` (the `__skill_invoked` meta-check uses the LLM judge), no permission-denials reader, no
  `[conversation]` (scripted `turns` evals are rejected).
- **Stray-writes caveat:** the audit reads top-level args keys (`command`, `path`, `filePath`,
  ...); Cline nests args under `input`, so with declarative extraction the audit classifies
  nothing for this harness — effectively the no-transcript baseline for that one audit until the
  named parser lands. The `[tools]` vocabulary is still declared (load-time validation requires
  it with `[transcript]`, and the future guard consumes it).

## Wiring the next enhancements

Tracked as the Cline-harness gap ticket
([#234](https://github.com/slowdini/eval-magic/issues/234)); each is a separate one-capability-per-PR code
contribution (see docs/progressive-enhancements.md "Guardrails"), in leverage order:

1. **`cline-json` named transcript parser** — new `TranscriptParser` variant + schema enum value
   + `src/adapters/cline/transcript.rs`: flatten `input` to top-level args per tool (the
   stray-writes audit then sees `command`/paths and the meta-check sees `skill`), join
   `content_start`/`content_end` by `toolCallId` to attach results, normalize per-tool output
   shapes (observed: `skills` results are strings, other tools' are `[{query, result, success}]`
   arrays), and add refusal recognition for `permission-denials.json`. Unlocks full-fidelity
   `transcript_check`, the deterministic `__skill_invoked` (`skill_tool = "skills"`,
   `skill_arg = "skill"`), and real stray-write coverage.
2. **`cline-skills` shadow preflight** — new `ShadowPreflight` variant scanning Cline's live
   roots: global `~/.cline/skills` (observed on 3.0.52; the docs' `~/.cline/data/settings/skills`
   path lags the binary) plus project-ancestor `.cline/skills`; check whether `.agents/skills`
   is also read (the cline repo carries one; unverified). Until it lands, no preflight runs and
   the run warning says so.
3. **Write guard engine arm** (`cline-plugin`, mirroring `opencode-plugin`) — stage a JS plugin
   at `.cline/plugins/` whose `tool_call_before` hook (blocking, `fail_closed`) forwards tool
   calls to `eval-magic guard-hook --harness cline`. Needs verification that project plugins
   auto-load in headless one-shot mode and of the exact deny-verdict shape from the SDK hooks
   docs. `CLINE_COMMAND_PERMISSIONS` is shell-command-only and not a guard substitute.
4. **Conversation resume** — blocked upstream: needs the resume `sessionId` surfaced in the
   `--json` stream *and* headless `--id` fixed (both absent/broken in 3.0.52, see "Dispatch
   quirks"). Then declare `[transcript.extract.session_id]` plus
   `[conversation].resume_exec_template` (`cline --cwd <eval-root> --id {session_arg} ...`).
