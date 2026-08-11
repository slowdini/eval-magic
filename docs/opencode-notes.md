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
| `harnesses/opencode.toml` | the descriptor — every declarative value + the `opencode` slug, `opencode-events` summary/denial, and `opencode-skills` shadow-preflight references |
| `harnesses/opencode-guard-plugin.js` | the embedded write-guard project plugin staged by the `opencode-plugin` guard engine (`{exe}`/`{marker}` substituted; byte-pinned in `src/adapters/guard.rs`) |
| `mod.rs` | slug sanitization/truncation (the `opencode` slug capability) |
| `transcript.rs` | `opencode run --format json` event-stream parsing (the `opencode-events` transcript capability) |
| `skill_shadow.rs` | project/global skill collision scan (`opencode-skills`) + reporting |

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
    rules are still enforced under `--auto`.
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
- **Shadow preflight:** the `opencode-skills` capability scans every root OpenCode discovers
  skills from and warns at build time when a staged logical skill is also live there — see
  "Isolating from live skills" below.
- **Conversation resume:** scripted follow-ups use
  `opencode run --dir <eval-root> --session <SESSION_ID>`; `sessionID` on the JSON event envelope
  supplies and verifies the native id. Verified against `opencode run --help` on 2026-07-24.
- **Write guard:** a project plugin staged at `.opencode/plugins/slow-powers-eval-guard.js` —
  see "Write guard" below.

## Write guard

A guarded run (the guard auto-arms; `--guard`/`--no-guard` make it explicit) stages a project
plugin at `.opencode/plugins/slow-powers-eval-guard.js` from the embedded template
`harnesses/opencode-guard-plugin.js` (`{exe}`/`{marker}` substituted as JSON string literals).
OpenCode auto-loads project plugins by directory convention — no trust prompt, no dispatch flag
(in contrast to codex's `--dangerously-bypass-hook-trust`). The plugin's `tool.execute.before`
hook forwards every tool call as `{"tool_name", "tool_input"}` on stdin to the generic
`eval-magic guard-hook --harness opencode <marker>` entry point and throws the verdict's
`reason` to block; the shared arbiter stays the single classification point (the plugin is
deliberately dumb). The deny verdict rides the shared renderer with the codex-style
`{"decision":"block","reason":"..."}` shape. Teardown deletes the plugin it created (restoring a
original file verbatim) and prunes `.opencode/plugins/` when restoring leaves it empty
(`guard_hook_cleanup_dir`).

Live-verified on v1.18.4 (#155):

- A staged `.opencode/plugins/*.js` auto-loads under headless `opencode run --dir <env>
  --format json --auto` with no trust prompt (a debug plugin logged `plugin initialized`).
- `tool.execute.before` fires under `--auto` — i.e., after permission auto-approval (every tool
  call logged, including `read` and `skill`).
- A thrown `Error` aborts the tool call and surfaces in the events stream as a tool part with
  `"status":"error"` carrying the message (`"error":"eval guard: canary deny"`); the blocked
  file was never created.
- In an armed env: an in-bounds `write` completed; an out-of-bounds `write` to `/tmp/...` was
  blocked with `"error":"eval guard: write to /tmp/... is outside the eval sandbox (allowed:
  ...). For temporary or scratch files, use <env>/tmp."`; an out-of-bounds bash redirect
  (`echo hi > /tmp/...`) was blocked with
  `"error":"eval guard: blocked bash (output redirection to a file) — runs outside the eval
  sandbox. For temporary or scratch files, use <env>/tmp."`; and `eval-magic teardown-guard`
  printed `🛡 Write guard removed.`, deleted the plugin, pruned `.opencode/plugins/`, and swept
  the marker.

Two boundary notes, shared with the other harnesses' guards:

- The marker's sole allowed root is the private task env on every host. Host temp locations such
  as `/tmp` and `$TMPDIR` remain out of bounds; dispatch prompts direct scratch work to
  `<env>/tmp/` instead without rewriting `TMPDIR`, `TMP`, or `TEMP`.
- Bash coverage is the shared heuristic denylist (installs, git mutations, redirects, config-dir
  tampering): a bare `touch /abs/outside/path` matches no pattern and is allowed — after-the-fact
  detection of those is `detect-stray-writes`' job, same as claude/codex.

One hook-shape caveat: `tool.execute.before` fires for *every* tool (OpenCode has no matcher
surface), so each tool call spawns one `eval-magic guard-hook`. Classification stays in the
arbiter, and tools outside the write/patch/shell vocabulary fall through to allow in
milliseconds — the same fail-open-per-call posture as the other engines.

## Permission denials

OpenCode has no dedicated refusal channel: a tool call the harness refuses to run is recorded as
an ordinary `tool_use` event whose `part.state.status` is `"error"` and whose `state.error` carries
the refusal explanation. The explicitly selected
`permission_denials_parser = "opencode-events"` reader tells a refusal from an ordinary tool error
by the *content* of that string, which OpenCode itself authors at the permission layer — before the
tool body runs — so matching it is not guessing from arbitrary result text:

- An **explicit deny rule** (the operator `permission` config matching the call) throws
  `PermissionDeniedError` with the fixed prefix
  `The user has specified a rule which prevents you from using this specific tool call.` followed by
  the operator's rule list as JSON. The parser drops that ruleset tail and keeps the prefix as the
  reason.
- A **headless reject** (an approval ask with nobody to approve it; or reject-with-feedback) throws
  `PermissionRejectedError`/`PermissionCorrectedError` with the prefix
  `The user rejected permission to use this specific tool call`. The parser normalizes both to the
  one canonical sentence and drops any user feedback. This path is unreachable in-eval (the dispatch
  recipe runs `--auto`, so asks auto-approve), but is recognized for robustness.
- The **eval write guard** throws the shared `eval guard: <reason>` string (the plugin throws the
  verdict's `reason` verbatim, see "Write guard" below); the parser keeps it verbatim so the
  pipeline can attribute it via the `eval guard: ` prefix and leave it to the guard's own warning
  rather than reporting it twice.

The report stores the refused input's *keys* — sorted, never values — so a refused `edit`/`write`
cannot spill a file body and a refused `apply_patch` cannot spill its patch. Ordinary tool errors
(a bash `command not found`, an `edit` `oldString not found`, DNS and OS-process failures) never
match those OpenCode-authored prefixes and are therefore not classified as permission denials.

A note on tool *visibility*: when an operator rule denies a tool with pattern `"*"` and action
`"deny"`, OpenCode removes that tool from the offered toolset entirely — the agent has no `bash`
to call and emits no event, so there is nothing to record. Pattern-specific denies (which keep the
tool visible so a matching call throws `PermissionDeniedError`) are what the reader captures; a
globally-denied tool is simply absent from the transcript.

These shapes were verified against `opencode v1.18.10` on 2026-07-30 by capturing
`opencode run --format json --auto` under a `permission.bash` deny rule. Re-check the parser
fixtures if OpenCode changes its permission-error wording.

## Isolating from live skills

Every `opencode run` discovers skills well beyond the eval env. Before dispatch, the
`opencode-skills` preflight compares each logical eval skill name against every root OpenCode
loads from (verified against the v1.18.3 skills doc and `packages/opencode/src/skill/index.ts`):

- `.opencode/skills`, `.claude/skills`, and `.agents/skills` at each directory level walked up
  from the dispatch cwd to the git worktree (the staged env's own dirs are excluded — they hold
  the intentional staged copies);
- `$XDG_CONFIG_HOME/opencode/skills` (default `~/.config/opencode/skills`);
- `$OPENCODE_CONFIG_DIR/skills` when set — **additive**, not a replacement for the xdg default;
- the legacy `~/.opencode/skills` (still scanned by OpenCode, though the docs omit it);
- `~/.claude/skills` and `~/.agents/skills`.

The `.claude`/`.agents` roots are a cross-harness contamination vector: a skill installed for
Claude Code or Codex is visible to OpenCode sessions by default. Direct skill directories are
matched by the `name:` in `SKILL.md` frontmatter, not the folder name. Missing directories and
malformed skills are ignored; a failure in one root never suppresses findings from the others.
The scan runs in every comparison environment. Schema-v2 `plugin-shadow.json` records native versus
cross-harness roots, canonical and discovery paths, logical and runtime names, every affected
cell, and per-source remediation. When live and staged sources share a runtime ID, preflight runs
`opencode debug skill` from that environment and records the selected versus shadowed path; a
failed or unparseable probe remains `unknown` rather than guessing. Unique runtime IDs need no
probe and are `selected`. The shared banner and `aggregate` validity warnings render this same
report; historical unversioned artifacts remain readable.

eval-magic detects but cannot unload these sources, and the generated dispatch recipes never
set the `OPENCODE_DISABLE_*` kill switches on the operator's behalf — parity with a real user
session matters. The operator-facing recipes — both switches with their exact scopes, and
move-or-rename as the only remedy for an `.opencode` root — are in the shipped
`eval-magic docs isolation` topic ([isolation guide](guides/isolation.md)).

When descriptor environment/recipes exclude **every** reported source from every initial and
resumed dispatch, an overlay may declare `[shadow] isolates_live_sources = true`. Preflight still
writes every source and the assertion to `plugin-shadow.json`; `run` prints an informational
notice and `aggregate` omits the shadow validity warnings. eval-magic does not inspect or verify
the dispatch configuration; the honesty rules, including which OpenCode remedies can and cannot
justify the assertion, are in `eval-magic docs isolation`.

**Known limits:** OpenCode also loads skills from config-declared `skills.paths` directories
and `skills.urls` (remote-pulled), and matches a singular `.opencode/skill/` directory; the
preflight does not scan those sources. Verify those cases manually when relevant. (Also stated for
operators in `eval-magic docs isolation`.)

## Naming rules

OpenCode skill names must be 1–64 characters, lowercase alphanumeric with single-hyphen separators
(no leading/trailing/consecutive hyphens), and match the containing directory name. `staged_slug`
sanitizes the generated slug while preserving the `slow-powers-eval-` cleanup prefix (truncating
the skill portion if the combination exceeds 64 chars); `validate_stage_name` applies the same
rules to `--stage-name` overrides. Sibling skills stage at their natural names and must already
satisfy the rules.
