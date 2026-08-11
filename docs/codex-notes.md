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
| `transcript.rs` | `codex-items` compatibility/reference summary parser + paired-stderr denial reader |
| `skill_shadow.rs` | repo/user/admin/plugin skill collision scan (`codex-skills`) + reporting |

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
- Scripted follow-ups run from `<eval-root>` through `codex exec resume <SESSION_ID> <PROMPT>`;
  `thread.started.thread_id` supplies the id and each round keeps `--json` plus its own
  `--output-last-message` capture. Verified against `codex exec resume --help` on 2026-07-24.

## Model flag

`-m <model>` is inserted before `--json` when `run --agent-model` is set. Judge recipes read each
task's resolved `model` from `judge-tasks.json` (`run --judge-model` sets the default; a
per-assertion `llm_judge.model` overrides it) and pass `-m "$model"` only when one is present.

## Staging & discovery

Skills stage under repo-local `.agents/skills/`. Codex keys discovery on the frontmatter `name:`,
so the staged skill-under-test's frontmatter is rewritten to the eval slug
(`rewrites_frontmatter_name` true) and the available-skills block advertises the slug
(`advertises_staged_slug_name` true).

Codex can also inject `--bootstrap` content into a no-stage dispatch: the bootstrap remains in
`<session-start-context>`, while the skill-under-test is inlined separately for the treatment arm.
`--stage-name` still requires staging because it names an on-disk staged copy.

## Isolating from live skills and plugins

Every `codex exec` can discover skills beyond the eval env. Before dispatch, the `codex-skills`
preflight compares each logical eval skill name with:

- `.agents/skills` at each repository ancestor of the dispatch cwd;
- `$HOME/.agents/skills`;
- `/etc/codex/skills`; and
- skills in enabled installed plugins reported by `codex plugin list --json`, under
  `$CODEX_HOME/plugins/cache/<marketplace>/<plugin>/<version>/skills`.

Direct skill directories are matched by the `name:` in `SKILL.md` frontmatter, not the folder
name. Missing directories, malformed skills, and unavailable or invalid plugin-list output are
ignored; a plugin-list failure does not suppress findings from the direct directories. The scan
runs in every comparison environment. Schema-v2 `plugin-shadow.json` records project/user/admin or
plugin roots, canonical and discovery paths, logical and runtime names, every affected cell, and
per-source remediation. Codex can expose same-name skills together, so duplicate runtime IDs are
recorded as `coexisting`. The shared banner and `aggregate` validity warnings render this same
report; historical unversioned artifacts remain readable.

`--disable plugins` is a **global** option, so it has to precede `exec` — for example
`codex --disable plugins --ask-for-approval never exec ...`; codex-cli rejects it after `exec`. By
default `plugin-shadow.json` and aggregate validity warnings retain the preflight finding because
eval-magic cannot observe manually added launch arguments. The operator-facing recipes — that flag,
move-or-rename for a direct skill collision, and the clean-`HOME`/`CODEX_HOME` caveat — are in the
shipped `eval-magic docs isolation` topic ([isolation guide](guides/isolation.md)).

When a descriptor overlay excludes **every** reported source from every initial and resumed
dispatch, it may declare `[shadow] isolates_live_sources = true`. Preflight and
`plugin-shadow.json` remain as auditable provenance, while `run` prints an informational notice
and `aggregate` omits the shadow validity warnings. eval-magic does not inspect the recipes or
otherwise verify the assertion; the honesty rules, including which Codex remedies can and cannot
justify it, are in `eval-magic docs isolation`.

**Known limit:** Codex also ships bundled system skills, but exposes no stable
enumeration mechanism for them. The preflight therefore cannot detect a collision with a bundled
system skill; verify that case manually when relevant. (Also stated for operators in
`eval-magic docs isolation`.)

## Transcript (`item.completed`)

The built-in descriptor normalizes the JSONL summary through `[transcript.extract]`; the
`codex-items` named parser remains as a compatibility/reference implementation, with a differential
test pinning the declarative output to it. Its separately selected denial reader still handles the
paired stderr capture described below.

`item.completed` events whose item type is not an agent message / reasoning / plan update become
tool invocations: `command_execution`, `file_change`, `web_search`, and MCP items.
`thread.started.thread_id` is normalized as the resumable session id, and every completed
`agent_message` is preserved in event order for conversation gating. `transcript_check` matches
these parsed items. The JSONL exposes **no deterministic skill-tool
event**, so `transcript_surfaces_skill_invocation()` is false and the `__skill_invoked` meta-check
uses the LLM-judge fallback.

Codex token totals use its blended workload metric:

```text
max(input_tokens + output_tokens - cached_input_tokens, 0)
```

`reasoning_output_tokens` is already a detail of `output_tokens`, so adding it again would double
count reasoning. Resumed `turn.completed` usage is cumulative for the native thread; the Codex
descriptor therefore uses the final round's total instead of summing round totals.

`codex exec --json` transcripts do not include a native duration or event timestamps, so
`duration_ms` remains `null`. Aggregate reports the missing sample count; a benchmark statistic
with `n: 0` is unavailable, not a measured zero. Ingest does not migrate timing artifacts
automatically. Run `eval-magic ingest --harness codex --iteration <N> --overwrite` to regenerate
them from preserved transcripts when desired.

### Permission denials

Some pre-execution refusals never become Codex JSONL items. The independently selected
`permission_denials_parser = "codex-items"` reader therefore derives `codex-stderr.log` from
`codex-events.jsonl` (and the equivalent paired filenames in scripted-turn directories) and
recognizes two structural Codex tool-router forms:

- `exec_command` `Rejected(...)` records for approval-required policy and explicit deny rules;
- `Command blocked by PreToolUse hook` records for `Bash` and patch-shaped `apply_patch` calls.

The report stores `input_keys: ["command"]` but never the command or patch body. Explicit-rule
command echoes and hook `. Command: ...` payloads are stripped from the reason; the
`eval guard: ` prefix remains exact so aggregate can attribute guard blocks without warning
twice. A missing stderr capture, unrelated/malformed lines, and ordinary JSONL command failures
produce no denial. In particular, DNS and OS-process failures are indistinguishable from normal
tool failures and are intentionally left unclassified to avoid false positives.

These forms were verified against `codex-cli 0.146.0` on 2026-07-30. Re-check the reader fixtures
when Codex changes its stderr logging shape.

## Write guard

A guarded run (the guard auto-arms; `--guard`/`--no-guard` make it explicit) merges a
`PreToolUse` hook into `.codex/hooks.json` (matcher:
`^Bash$|^apply_patch$|^Edit$|^Write$`, with a 30s timeout and status message). Dispatches must pass
`--dangerously-bypass-hook-trust` so the vetted project-local hook actually runs — the generated
eval-agent recipes add it whenever the run was armed. Judge recipes never add the flag: judges
run from the iteration metadata directory, outside the guarded task envs.

Codex exposes the raw patch body as `tool_input.command` (the contract landed in
[openai/codex#18391](https://github.com/openai/codex/pull/18391)). The arbiter extracts every add,
update/delete source, and move destination from that body and resolves relative paths from the
hook payload's `cwd`. Bash output validation uses a quote-aware lexical scan for `>`, `>>`, `>|`,
file-descriptor-prefixed redirects, and `tee`: every literal target must resolve under an allowed
root, while dynamic, malformed, or outside targets are blocked. Merely mentioning an allowed root
elsewhere in the command does not scope an unrelated redirect.

Guard installation initializes `.eval-magic-outputs/guard-denials.jsonl` and records its absolute
path in the optional marker field `denialLogPath`. Each block appends only timestamp, harness,
tool, reason, resolved targets, and sorted input keys — never patch or command content. Re-arming
truncates the log; guard teardown preserves it; a logging failure never suppresses the block.
`detect-stray-writes` joins the raw logs to dispatch tasks in `guard-denials.json`, and
`aggregate` reports one validity warning per affected task.

The hook invokes the hidden `guard-codex` subcommand
(**stable on-disk contract — never rename**), which blocks via Codex's
`{ "decision": "block", "reason": "..." }` stdout shape and stays silent to allow. Teardown prunes
`.codex/` when restoring the original config leaves it empty (`guard_hook_cleanup_dir`).

## Running inside Codex itself

User-facing guidance lives in `eval-magic run --help`: staging writes `.agents/skills`, guarded
runs also write
`.codex/hooks.json`, and Codex's default workspace-write sandbox protects those paths, so the
runner may need approval/escalation or an external terminal invocation. Keep that help text in sync
when this changes.
