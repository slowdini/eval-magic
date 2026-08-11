# Claude Code — harness implementation notes

> **Audience:** developers working on eval-magic's Claude Code support. Runtime usage lives in the
> README, `--help`, and the generated `RUNBOOK.md`; the enhancement model is in
> [progressive-enhancements.md](progressive-enhancements.md).

## Code map

The declarative half (label, dirs, phrases, command templates, banner) is the descriptor file
`harnesses/claude-code.toml`; `src/adapters/claude_code/` keeps only the code capabilities the
descriptor references:

| File | What's in it |
|------|--------------|
| `harnesses/claude-code.toml` | the descriptor — every declarative value + capability references |
| `stream_json.rs` | `claude-stream-json` summary/denial reader + surface compatibility reference |
| `transcript.rs` | JSONL record shapes + shared tool-call extractors |
| `plugin_shadow.rs` | plugin-shadow detection + isolation banner (`claude-plugins`) |

The write guard has no per-harness code: the descriptor's `[guard]` block (hook file, matcher,
hook-entry and `hookSpecificOutput` verdict templates) is rendered by the generic engine in
`src/adapters/guard.rs`; the hidden `guard` subcommand is its frozen hook entry-point alias.

## Dispatch quirks (all forced by the `claude` CLI)

- `--output-format stream-json` **requires `--verbose`** in `-p` mode.
- There is **no `--cd` flag**: every dispatch must run from its env dir (`cd <eval-root> &&`).
  Staged-skill discovery is cwd-relative, so getting this wrong makes the `with_skill` arm behave
  like `without_skill`.
- There is **no `--output-last-message`**: the final message is recovered from the stream-json
  `result` event rather than a file.
- `</dev/null` detaches stdin so a permission prompt can't block on a TTY and piped task data
  can't become extra prompt context.
- Dispatches use **`--permission-mode bypassPermissions`**, not `acceptEdits`. See below.
- Scripted follow-ups use `claude -p --resume <SESSION_ID>` from the same env; the initial
  `system.session_id` supplies the id. Verified against `claude --help` on 2026-07-24.

## Permission mode

Every dispatch and judge recipe carries `--permission-mode bypassPermissions`. The obvious
alternative, `acceptEdits`, is wrong here: it auto-approves *file edits* but **not Bash**, and
because the recipe detaches stdin (`</dev/null`) there is nobody to approve, so anything not
trivially safe is auto-denied. Measured on a real dispatch, `ls`/`grep`/`find` ran while
`bun run repro.ts`, `node -e '…'` and even `bun --version` came back "This command requires
approval".

That failure is invisible: nothing errors, no warning is emitted, and the run grades normally.
For any skill whose behavior involves *running* things — reproducing a bug, running a test suite,
verifying a fix — both arms silently degrade to static reasoning, and a `transcript_check` can
still pass on an attempt that never executed, because the tool call is recorded whether or not it
was permitted. This is the same posture the other built-ins already take: Codex dispatches with
`--ask-for-approval never --sandbox workspace-write`, OpenCode with `--auto`.

**The write guard, not the permission mode, is the boundary.** The `PreToolUse` hook still fires
under `bypassPermissions` and its deny verdicts are still enforced; `--guard`'s own help describes
the guard as a backstop for exactly this case, "when the isolated session runs with relaxed
permissions". With `--no-guard` there is no enforcement boundary at all — that is the trade the
flag makes.

`bypassPermissions` is refused in some environments (running as root, or when managed settings
disable it). Override the mode for those hosts with a `--harness-file` descriptor that retunes the
four `[dispatch]`/`[conversation]` templates; field-level merge means nothing else has to be
restated.

Relaxing the default closes the common case, not the class — a deny rule, a managed setting, or an
operator-overridden mode still refuses calls — so refusals are detected and reported rather than
assumed away (see "Permission denials" below).

## Permission denials

Verified against `claude` **2.1.220**:

- The terminal `result` event carries a structured `permission_denials` array —
  `{"tool_name","tool_use_id","tool_input"}` per refused call. No refusal-text matching needed.
- The matching `tool_result` block carries the refusal text with `is_error: true` (e.g.
  `"This command requires approval"`), recovered by `tool_use_id` as the denial's `reason`.
- **A `PreToolUse` hook deny also populates `permission_denials`, including under
  `--permission-mode bypassPermissions`** — probed with a deny hook, whose
  `permissionDecisionReason` came back as the `tool_result` content. The eval write guard denies
  exactly that way, so its blocks appear here too and are attributed by the `eval guard: ` reason
  prefix so `aggregate` does not warn about one denial twice.
- Builds predating the field simply omit it, which degrades to "no denials reported".

`ingest` turns this into `permission-denials.json` and `aggregate` into one validity warning per
affected task; see [progressive-enhancements.md](progressive-enhancements.md).

## Transcript (stream-json)

`outputs/claude-events.jsonl` is the `-p` stream-json stream. `assistant`/`user` events wrap full
Anthropic Messages objects (tool-call extraction matches `tool_result` blocks back to their
`tool_use` by id); a terminal `result` event carries the authoritative final text, wall-clock
duration, and token usage — there are no per-line timestamps. `system`, `rate_limit_event`, and
other non-message events are skipped. The transcript exposes Skill-tool invocations, so the
`__skill_invoked` meta-check is deterministic here.

The built-in descriptor uses the named parser for this cross-event summary, selects its denial
reader explicitly, and maps the session roster through the generic
`[transcript.extract.session_surface]` block. A differential test keeps that mapping aligned with
the named parser's retained compatibility implementation.

The session-opening `{"type":"system","subtype":"init"}` event reports what the dispatch actually
loaded, which is what `eval-magic docs isolation` steers operators to for verifying isolation.
Verified against 2.1.220/2.1.223:

- `plugins` is an array of `{name, path, source, version?}`. `source` (`"slow-powers@slowdini"`) is
  byte-identical to the `enabledPlugins` key `plugin_shadow.rs` scans; `name` (`"slow-powers"`) is
  the namespace it derives. `version` is absent for some installs.
- `skills` lists advertised skill ids, built as
  `skills.filter(s => s.userInvocable !== false).map(s => s.name)`. Plugin skills appear as
  `<plugin-name>:<skill>` — exactly the `runtime_id` `plugin_shadow.rs` synthesizes. Staged skills
  appear under their staging *directory* name, not the frontmatter `name:`.
- **`init` is not the first line.** A capture opens with `subtype: "hook_started"` when a hook is
  installed, which the guard always is. Anything reading the init record must filter on
  `subtype == "init"`, not `type == "system"` alone — `parse_claude_stream_json_full`'s `session_id`
  scan matches only on the latter and happens to be safe because both events carry the same id.
- Resumed turns (`--resume`) emit their own full `init` event, so per-turn evidence exists.

## Skill discovery & staging

Staged skills live at `.claude/skills/` in each env; discovery is structural and cwd-relative, and
envs are fully built before any dispatch runs, so there is no mid-session staging hazard. The Skill
tool resolves the staged directory name directly: the frontmatter `name:` is **not** rewritten
(`rewrites_frontmatter_name` is false) and the natural name is advertised.

## Isolating from installed plugins

Each `claude -p` dispatch loads the user/global plugins and skills from its Claude config. The
staging slug prevents an on-disk collision but not runtime discovery — an installed plugin exposing
a same-named skill is discoverable in *both* arms, so the control arm is not truly skill-absent.
`plugin_shadow.rs` detects this in every comparison environment. The shared shadow policy records
one finding per logical skill in schema-v2 `plugin-shadow.json`, including every affected cell,
canonical/discovery paths, source-specific remediation, and the runtime identifier the agent sees.
Claude plugin skills use their namespaced `<plugin>:<skill>` runtime ID, direct live skills retain
the logical name, and staged subjects use their staging-directory slug. Direct live duplicates
record user-before-project precedence; a staged subject with its distinct slug remains selected.
The shared banner and `benchmark.json` `validity_warnings` consume the same report. The runner can
detect but never unload a live plugin. The remediation options (also printed inline in the banner):

The three remedies the banner names — `--setting-sources project,local`, a per-plugin
`"enabledPlugins": { "<plugin>@<marketplace>": false }`, and a clean
`CLAUDE_CONFIG_DIR="$(mktemp -d)"` — are documented for operators, with the caveat attached to each
(including the OAuth caveat for a relocated config dir), in the shipped `eval-magic docs isolation`
topic ([isolation guide](guides/isolation.md)). The per-source strings the banner prints live in
`plugin_shadow.rs`; keep them consistent with that topic.

`--setting-sources project,local` drops **all** user-scope discovery, not just `enabledPlugins`:
skills under `<config_dir>/skills` are unloaded too. Verified 2026-08-06 by A/B within one campaign —
the judge recipe carries no `--setting-sources` and its capture lists both `~/.claude/skills`
entries and every `<plugin>:<skill>` id, while all 48 isolated eval dispatches list neither.

Project-local staged skills are independent of installed plugins, so they still load and the
meta-check still resolves the slug under all three options.

When a descriptor overlay applies one of these remedies to every initial and resumed dispatch, it
may declare `[shadow] isolates_live_sources = true`. Preflight still detects and writes every
source to `plugin-shadow.json`, along with the assertion, but `run` prints an informational notice
and `aggregate` omits the findings from `validity_warnings`. eval-magic does not verify the claim or
inspect the dispatch templates. The honesty rules and the per-harness traps are in
`eval-magic docs isolation`.

## Write guard

A guarded run (the guard auto-arms; `--guard`/`--no-guard` make it explicit) merges a
`PreToolUse` hook into each env's `.claude/settings.local.json` (matcher:
`Write|Edit|MultiEdit|NotebookEdit|Bash`). Every dispatch runs from its env, so it loads and
enforces the hook — the recipe never passes `--bare`, which would skip hook discovery. It is the
only write boundary a dispatch has, since the session itself runs under `bypassPermissions` (see
"Permission mode"). The hook
invokes the hidden `guard` subcommand (**stable on-disk contract — never rename**), which denies
via Claude Code's `hookSpecificOutput` JSON shape and stays silent to allow. Both layers fail open.
A deny aborts the offending dispatch; `detect-stray-writes` remains the after-the-fact backstop.
