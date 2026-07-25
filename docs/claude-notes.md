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
| `stream_json.rs` | `-p --output-format stream-json` transcript parsing (`claude-stream-json`) |
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
- Scripted follow-ups use `claude -p --resume <SESSION_ID>` from the same env; the initial
  `system.session_id` supplies the id. Verified against `claude --help` on 2026-07-24.

## Transcript (stream-json)

`outputs/claude-events.jsonl` is the `-p` stream-json stream. `assistant`/`user` events wrap full
Anthropic Messages objects (tool-call extraction matches `tool_result` blocks back to their
`tool_use` by id); a terminal `result` event carries the authoritative final text, wall-clock
duration, and token usage — there are no per-line timestamps. `system`, `rate_limit_event`, and
other non-message events are skipped. The transcript exposes Skill-tool invocations, so the
`__skill_invoked` meta-check is deterministic here.

## Skill discovery & staging

Staged skills live at `.claude/skills/` in each env; discovery is structural and cwd-relative, and
envs are fully built before any dispatch runs, so there is no mid-session staging hazard. The Skill
tool resolves the staged directory name directly: the frontmatter `name:` is **not** rewritten
(`rewrites_frontmatter_name` is false) and the natural name is advertised.

## Isolating from installed plugins

Each `claude -p` dispatch loads the user/global plugins and skills from its Claude config. The
staging slug prevents an on-disk collision but not runtime discovery — an installed plugin exposing
a same-named skill is discoverable in *both* arms, so the control arm is not truly skill-absent.
`plugin_shadow.rs` detects this at build time and surfaces it as the shadow banner plus
`benchmark.json` `validity_warnings`; the runner can detect but never unload a live plugin. The
remediation options (also printed inline in the banner):

- **Drop user-scope plugins, keep auth:** add `--setting-sources project,local` to the dispatch.
  User-scope `enabledPlugins` isn't loaded; auth is unaffected.
- **Disable the specific plugin:** set `"enabledPlugins": { "<plugin>@<marketplace>": false }` in a
  settings source the dispatch loads.
- **Clean config dir (strips everything):** run each dispatch under
  `CLAUDE_CONFIG_DIR="$(mktemp -d)"`. No installed plugins or global skills load. Auth caveat:
  OAuth lives in `~/.claude.json`, which a relocated config dir may not carry — set
  `ANTHROPIC_API_KEY` or re-authenticate once in the fresh dir.

Project-local staged skills are independent of installed plugins, so they still load and the
meta-check still resolves the slug under all three options.

## Write guard

A guarded run (the guard auto-arms; `--guard`/`--no-guard` make it explicit) merges a
`PreToolUse` hook into each env's `.claude/settings.local.json` (matcher:
`Write|Edit|MultiEdit|NotebookEdit|Bash`). Every dispatch runs from its env, so it loads and
enforces the hook — the recipe never passes `--bare`, which would skip hook discovery. The hook
invokes the hidden `guard` subcommand (**stable on-disk contract — never rename**), which denies
via Claude Code's `hookSpecificOutput` JSON shape and stays silent to allow. Both layers fail open.
A deny aborts the offending dispatch; `detect-stray-writes` remains the after-the-fact backstop.
