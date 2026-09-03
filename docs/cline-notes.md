# Cline — harness implementation notes

> **Audience:** developers working on eval-magic's Cline support. Runtime usage lives in the
> README, `--help`, and the generated `RUNBOOK.md`; the enhancement model is in
> [progressive-enhancements.md](progressive-enhancements.md). The don't-guess rule
> ([byoh guide](guides/byoh.md)) requires every descriptor value to trace to Cline's own
> documentation or to observed output — that evidence is recorded here.

## Verified against

- Harness CLI + version: `cline 3.0.52` (`cline --version`), installed via npm (`npm i -g cline`);
  the `cline-json` parser re-verified every stream shape against `cline 3.0.53` on 2026-08-12
- Date verified: 2026-08-11 (descriptor), 2026-08-12 (parser)
- Documentation consulted: <https://docs.cline.bot/cline-cli/overview>,
  <https://docs.cline.bot/cline-cli/cli-reference>, <https://docs.cline.bot/customization/skills>,
  <https://docs.cline.bot/customization/plugins>, <https://docs.cline.bot/sdk/plugins>,
  <https://agentskills.io/specification>
- Observed output: a real `cline --json --auto-approve true` dispatch in a throwaway directory
  (676-line NDJSON capture exercising `read_files`, `run_commands`, and two `skills`
  invocations), `cline history --json`, several `--id` resume attempts, and
  `eval-magic harness lint harnesses/cline.toml --as-builtin --probe --yes` (the live probe
  rendered the exec template, dispatched it, parsed the event capture, and recovered a non-empty
  final response).
  Against 3.0.53: a second dispatch exercising `editor`, `run_commands`, `read_files`, and
  `skills` (arg/result shapes below), and a guard spike — a hand-staged `.cline/plugins/` project
  plugin whose `beforeTool` hook blocked calls, proving headless plugin auto-load, the
  `{skip, reason}` deny contract, `spawnSync` from the plugin sandbox, and the refusal stream
  shape (`content_end` output `{"error": "<reason>"}`).

## Code map

Cline support is the descriptor `harnesses/cline.toml` (the mechanical `EMBEDDED_DESCRIPTORS`
entry in `src/adapters/descriptor.rs`) plus `src/adapters/cline/`: `transcript.rs` holds the
`cline-json` named parser (arg flattening, `toolCallId` joins, per-tool result coercion, refusal
recognition). Shadow preflight, guard, and conversation remain descriptor-absent named-capability
gaps — see "Wiring the next enhancements" below.

## Verification record

Every non-comment field in `harnesses/cline.toml`, with its source — plus the parser behaviors
the descriptor references. "Probe capture" refers to the observed dispatches described above.

| Descriptor field / parser behavior | Value | Source |
|------------------|-------|--------|
| `label` | `cline` | chosen name |
| `skills_dir` | `.cline/skills` | docs CLI reference ("Configuration Files": project `.cline/skills/`); confirmed by probe capture: a skill staged at `<cwd>/.cline/skills/kebab-probe-skill` was invoked via `--cwd` alone |
| `config_dirs` | `[".cline"]` | same docs section (project config root is `.cline/`: rules/skills/hooks/plugins/mcp.json) |
| `run.supports_guard` | `true` | the `[guard]` table is present (the two move in lockstep); the arm itself is spike-verified on 3.0.53 (the `guard.*` rows) |
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
| `transcript.parser` / `permission_denials_parser` | `cline-json` / `cline-json` | args nest under `input` and `content_start`/`content_end` pair by `toolCallId` — beyond the extract tier's primitives, so the named parser does the normalization (3.0.53 capture) |
| `transcript.surfaces_skill_invocation` / `skill_tool` / `skill_arg` | `true` / `skills` / `skill` | 3.0.53 capture: `skills` calls carry `input.skill = "<name>"`, which the parser hoists to top level so the deterministic meta-check matches |
| Parser arg flattening | `run_commands` `commands:[...]` → one `command` string (newline-joined); every other tool's `input` hoists verbatim | 3.0.53 capture: `editor` takes `path`/`new_text`, `read_files` `files:[{path}]`, `skills` `skill`, `run_commands` `commands:[...]`; the join gives the stray-writes audit and guard a classifiable `command` |
| Parser result coercion | string (`skills`) / single `{query,result,success}` object (`editor`) / arrays of those objects (`run_commands`, `read_files`) / `{"error"}` on refusals → result text | 3.0.53 capture + guard spike (`beforeTool` block landed as `output:{"error":"<reason>"}`) |
| Parser denial recognition | `content_end` error payloads carrying the guard `eval guard: ` prefix (verbatim) or the runtime's `Tool … is disabled by policy` / `was not approved` / `was blocked by a runtime hook` wordings | guard-spike capture for the shape; the policy wordings are the runtime's fixed strings in the 3.0.53 binary |
| Parser summary fields | final text + token totals (cache reads subtracted, codex accounting) + `durationMs` from the terminal `run_result`; assistant messages from `content_end` text blocks (complete, ordered; `content_start` text chunks are streaming partials) | 3.0.52 + 3.0.53 probe captures |
| `model.flag` | `-m` | `cline --help`: `-m, --model <model-id>` |
| `guard.engine` / `plugin_file` | `cline-plugin` / `.cline/plugins/slow-powers-eval-guard/index.js` | 3.0.53 spike: project plugin *dirs* auto-load from `.cline/plugins/` in headless one-shot dispatches (a bare `index.js` needs no package.json; a loose `.js` at the plugins root is ignored) |
| `guard.verdict_template` | `{"decision":"block","reason":"{reason}"}` | same shape as the other engines; the staged plugin parses `reason` and returns it as `{skip: true, reason}` — 3.0.53 spike: the reason reaches the stream as the `content_end` `{"error"}` payload |
| `guard.armed_message` | see descriptor | prose authored for eval-magic output (same structure as the other built-ins) |
| Plugin hook contract | `beforeTool({snapshot, tool, toolCall, input})`; block with `{skip: true, reason}`; 3000ms default hook budget (plugin spawns with a 2s timeout so a hung arbiter fails open); `spawnSync` works from the plugin sandbox | 3.0.53 spike capture + the binary's runtime hook loop; the docs' `tool_call_before`/`fail_closed` vocabulary lags the binary |
| `shadow.preflight` | `cline-skills` | 3.0.53 root probe (one uniquely-named skill per candidate root): dispatch cwd's `.cline/skills` read, ancestor's NOT (no project walk), `~/.agents/skills` IS read (and receives `cline skill install` global installs); `$CLINE_DIR` overrides the `~/.cline` default (3.0.53 binary) |
| `dispatch.exec_template` | see descriptor | flags from `cline --help` (`--act` from the 3.0.52 binary’s hidden option registration + behavioral write test); `--json` NDJSON stdout and `</dev/null` stdin detach from the docs CLI overview (piped stdin becomes prompt context); final-text extraction verified by the live probe. `eval-magic dispatch --judges` reuses this same template, bound to the iteration directory and the judge prompt |
| `dispatch.next_steps_template` / `manifest_template` | see descriptor | prose authored for eval-magic artifacts (same structure as the other built-ins) |

## Dispatch quirks

- Cline's own plan mode is not declared as `[plan_mode]`: without session resume a plan could never
  be approved and implemented in one session, so `run` rejects plan-mode evals on Cline as it does
  every multi-turn eval.

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
- The terminal `run_result` NDJSON event carries the final text. The harness probe and ingest both
  recover it from the event file through the configured transcript parser; no separate completion
  file or jq post-pass is needed.
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
- **`cline-json` transcript ingest**: tool invocations with flattened top-level args
  (`run_commands`' `commands` array joins into one `command`; `editor` surfaces `path`;
  `skills` surfaces `skill`), results attached from the paired `content_end` (per-tool shape
  coercion, `{"error"}` payloads included), final text from `run_result.text`, ordered assistant
  messages from `content_end` text blocks, token totals from `run_result.usage` (cache reads
  subtracted), duration from `run_result.durationMs`. `transcript_check` patterns match the
  `"<name> <compact-json-args>"` rendering of the *flattened* args (e.g. `run_commands.*"command"`).
  `run_commands`, `editor`, and the read tools also answer to every other harness's spelling of
  their role, so a pattern written as `Bash` or `Read` grades here too.
- **Deterministic `__skill_invoked`**: `surfaces_skill_invocation = true` with
  `skill_tool = "skills"` / `skill_arg = "skill"` — the parser hoists the slug to top level, so
  the meta-check grades from the transcript instead of the LLM-judge fallback.
- **Permission denials**: the parser reads refusal evidence (`content_end` `{"error"}` payloads
  with the guard prefix or the runtime's policy wordings) into `permission-denials.json`.
- **`cline-skills` shadow preflight**: scans the two live global roots a dispatch can actually
  see — `$CLINE_DIR/skills` (default `~/.cline/skills`, native) and `~/.agents/skills`
  (cross-harness; `cline skill install` lands global installs there). Root verification
  (3.0.53 live probe, one uniquely-named skill per candidate root): the dispatch cwd's
  `.cline/skills` is read (runner staging during evals), an ancestor's `.cline/skills` is NOT
  (no project walk, unlike OpenCode), and `~/.agents/skills` IS read at runtime.
- **Stray-writes coverage**: flattened args give the audit top-level `command`/`path` keys, so
  write/shell classification works. Known blind spot: `read_files`' `files:[{path}]` stays
  nested, so the live-source-read path branch doesn't fire for it (shell-based read detection
  still covers `cat`-style reads).
- **No native conversation resume**: no `[conversation]`, so scripted `turns` and responder evals
  are rejected.
- **Write guard** (`cline-plugin` engine): `run` auto-arms a staged project plugin at
  `.cline/plugins/slow-powers-eval-guard/index.js` whose `beforeTool` hook forwards every tool
  call to `eval-magic guard-hook --harness cline` (`run_commands`' `commands` array joined into
  one `command` string for the shared arbiter) and returns `{skip: true, reason}` on deny.
  The shared cwd-aware policy allows ordinary installs, builds, tests, and in-place edits inside
  the task env while denying recognized explicit destinations outside it, output escapes,
  repository-routing escapes, and remote Git mutations.
  Spike-verified on 3.0.53 (all in a throwaway dir, hand-staged plugin): project plugin dirs
  auto-load in headless one-shot dispatches (a bare `index.js` needs no package.json; a loose
  `.js` file at the plugins root is IGNORED); the hook context is `{snapshot, tool, toolCall,
  input}` with `toolCall.toolName`/`toolCall.input`; `{skip, reason}` blocks and the reason
  reaches the stream as the `content_end` `{"error"}` payload; `spawnSync` works from the
  plugin sandbox (2s spawn timeout, under the runtime's 3000ms hook budget, so a hung arbiter
  fails open). The docs' hook vocabulary (`tool_call_before`, `fail_closed`) lags the binary —
  the plugin registers `beforeTool`. Teardown removes the plugin and prunes the plugin dir,
  restoring any pre-existing `index.js` verbatim.

## Wiring the next enhancements

Tracked as the Cline-harness gap ticket
([#234](https://github.com/slowdini/eval-magic/issues/234)); each is a separate one-capability-per-PR code
contribution (see docs/progressive-enhancements.md "Guardrails"), in leverage order:

1. ~~**`cline-json` named transcript parser**~~ — **landed**: `TranscriptParser::ClineJson` +
   `src/adapters/cline/transcript.rs` (verified against a fresh 3.0.53 capture, not the docs).
2. ~~**`cline-skills` shadow preflight**~~ — **landed**: `ShadowPreflight::ClineSkills` +
   `src/adapters/cline/skill_shadow.rs`. Roots verified against 3.0.53 with a live
   uniquely-named-skill probe: global `~/.cline/skills` (observed on 3.0.52; `$CLINE_DIR`
   override from the 3.0.53 binary) and `~/.agents/skills` (IS read at runtime — the ticket's
   open question — and is where `cline skill install` lands global installs); project
   `.cline/skills` is read only at the dispatch cwd (no ancestor walk), so the preflight scans
   no project roots.
3. ~~**Write guard engine arm** (`cline-plugin`)~~ — **landed**: `GuardEngine::ClinePlugin` +
   `harnesses/cline-guard-plugin.js` staged at `.cline/plugins/slow-powers-eval-guard/index.js`.
   Every ticket open item was spike-verified on 3.0.53 (see "What's wired"): headless project
   plugin auto-load, the `{skip, reason}` deny contract, the `{"error"}` stream shape (which the
   `cline-json` parser reads as a guard-attributed denial), and `spawnSync` from the plugin
   sandbox. `CLINE_COMMAND_PERMISSIONS` appears nowhere in the 3.0.53 binary — docs-only, and
   shell-only besides: not a guard substitute.
4. **Conversation resume** — blocked upstream: needs the resume `sessionId` surfaced in the
   `--json` stream *and* headless `--id` fixed (both absent/broken in 3.0.52, and `--id` still
   errors identically in 3.0.53 — re-verified 2026-08-12; see "Dispatch quirks"). Then declare
   `[transcript.extract.session_id]` plus
   `[conversation].resume_exec_template` (`cline --cwd <eval-root> --id {session_arg} ...`).
