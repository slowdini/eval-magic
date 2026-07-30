# Bring your own harness

> **Audience:** anyone (human or agent) pointing eval-magic at a harness it has never seen. No Rust
> required — a harness declares its capabilities in a TOML descriptor file, and every capability it
> *doesn't* declare degrades to a documented fallback with a warning, never a rejection. The
> baseline-vs-enhancement contract behind this is
> [progressive-enhancements.md](progressive-enhancements.md).

## The five-minute version

From inside a project that dispatches through `cool-custom-harness`:

```sh
eval-magic harness init cool-custom-harness
# → .eval-magic/harnesses/cool-custom-harness.toml (a commented template, lint-clean as
#   written) + cool-custom-harness-notes.md (the verification-record skeleton).
# Fill in verified values following the template's inline comments, then:
eval-magic harness lint .eval-magic/harnesses/cool-custom-harness.toml
eval-magic harness list
eval-magic run --harness cool-custom-harness
```

The scaffold registers the name as a baseline harness before a single field is filled in; the
first field worth uncommenting is `[dispatch] exec_template`, e.g.:

```toml
label = "cool-custom-harness"

[dispatch]
exec_template = '''
cool-cli run --cd <eval-root>{model_arg} \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly." \
  > <outputs_dir>/final-message.md'''

[dispatch.env]
TZ = "UTC"
```

That run is complete: `run` warns about each enhancement the descriptor doesn't declare (naming the
fallback that carries it), builds `dispatch.json` / `RUNBOOK.md` / `dispatch-manifest.md` with your
exec recipe, and after you dispatch the tasks, `ingest` assembles records from each task's
`outputs/final-message.md`, captures final-environment metrics in `diff-scope.json`, runs the
`detect-stray-writes` audit against the task's `eval_root`, executes any runner-owned
`command_check` assertions, and hands soft grading to `llm_judge`.

For a one-off descriptor that shouldn't live in the project, pass it directly — its label becomes
the invocation's default harness, so `--harness` can be omitted:

```sh
eval-magic run --harness-file ./cool-custom-harness.toml
```

## The minimum viable descriptor

`label` alone loads:

```toml
label = "cool-custom-harness"
```

That is a **baseline** harness: staging is unavailable (runs fall back to `--no-stage`, inlining
each `SKILL.md` into its dispatch prompt), and dispatch guidance is generic. Add an
`[dispatch] exec_template` and the generated `RUNBOOK.md` / `dispatch-manifest.md` carry a
copy-pasteable per-task recipe instead.

### The exec-template contract

`exec_template` is the one-shot command that dispatches a single task. eval-magic substitutes
`{model_arg}` (the harness's model flag + requested model, empty unless `[model].flag` is declared)
and `{guard_args}` (guard-only arguments, built-ins only); the `<eval-root>`,
`<dispatch_prompt_path>`, and `<outputs_dir>` placeholders are filled per task by whoever dispatches
(the runbook explains this). Two hard requirements:

1. **Run from the task's `eval_root`** — each `(eval, condition, run)` owns a private env and uses
   it as the subprocess cwd. Task source edits belong there; framework artifacts belong under the
   supplied `<outputs_dir>`.
2. **Recover the final message** — the agent's final reply must land in
   `outputs/final-message.md`. If your CLI can't write it directly (like the example above
   redirecting stdout), capture it yourself before running `ingest`.

Scripted evals additionally require `[conversation].resume_exec_template`. The driver fills
shell-quoted `{session_arg}` / `{prompt_arg}` values and per-round `<eval-root>` /
`<outputs_dir>` paths; the command must resume rather than fork that session. This capability also
requires transcript extraction of ordered assistant messages and the native session id. There is
no baseline fallback: `run` rejects `turns` for a harness that cannot preserve their conversational
meaning. Per-round token totals are summed by default. Set
`token_usage_aggregation = "last"` in `[conversation]` only when the native CLI reports cumulative
session usage on every resumed turn.

`harness lint <name|file> --probe` proves both hard requirements end-to-end before a real run —
it renders `exec_template` exactly as `run` would and asserts that `outputs/final-message.md` is
recovered afterwards. See the workflow section below and `harness lint --help`.

`[dispatch.env]` provides non-secret environment defaults for eval-agent subprocesses. Descriptor
layers merge this table by key; repeatable `run --agent-env KEY=VALUE` entries override the
resolved descriptor, with the last duplicate CLI entry winning. Values may be empty or contain
`=`, while names use portable shell identifier syntax. Git routing variables are reserved so they
cannot escape the task repository. The resolved map is recorded in clear text in
`conditions.json` and `dispatch.json`, applies to one-shot and scripted rounds, and does not apply
to judge agents or runner-owned `command_check` assertions. Unset keys inherit the operator's
environment; eval-magic does not impose a timezone default.

## Layering and field-level merge

Descriptor files stack in precedence order; **later layers override individual fields** of an
earlier descriptor with the same `label` (tables merge key-by-key; scalars and arrays replace
wholesale — never whole-file shadowing). A new `label` defines a new harness.

1. **Embedded built-ins** — `claude-code`, `codex`, `opencode` (bundled in the binary).
2. **User-global** — `<config-root>/harnesses/*.toml`, where the config root is
   `$EVAL_MAGIC_CONFIG_DIR` (empty value disables the layer), else `$XDG_CONFIG_HOME/eval-magic`,
   else `~/.config/eval-magic`.
3. **Project-local** — `<cwd>/.eval-magic/harnesses/*.toml`.
4. **`--harness-file <path>`** — a one-off top layer; when `--harness` is omitted its label is the
   invocation's default harness.

Files within one directory apply in filename order; one file per label per layer (a duplicate is
skipped with a warning). So a project can retune a single built-in field without restating the
descriptor:

```toml
# .eval-magic/harnesses/claude-code.toml — override one field of a built-in
label = "claude-code"

[model]
flag = "--model-x"
```

`eval-magic harness show claude-code` prints the resolved post-merge descriptor (as authorable
TOML, headed by the contributing files) — the fastest way to see what a layer actually changed.
Two caveats: the merge can't *delete* (an override can replace a field's value but cannot remove
an embedded table), and a guarded built-in's output isn't copy-paste-safe as a user layer until
you drop its `[guard]` table and `run.supports_guard` (see the guard restriction below).

**Broken discovered files never brick the CLI** — they are skipped with a warning pointing at
`eval-magic harness lint <file>`. A broken `--harness-file` is a hard error (you named it
explicitly).

## Field reference

The authoritative field-by-field reference is the bundled schema,
[`schema/harness-descriptor.schema.json`](../schema/harness-descriptor.schema.json); the semantics
and fallbacks per enhancement are in
[progressive-enhancements.md](progressive-enhancements.md), and `harness init` writes the same
map as inline comments in its scaffolded template. The short map:

| Table | Declares | Fallback when absent |
|-------|----------|----------------------|
| (top level) | `label` (required), `skills_dir`, `config_dirs` | no `skills_dir` ⇒ forced `--no-stage`, SKILL.md inlined |
| `[dispatch]` | exec/parallel/judge/next-steps/manifest templates; eval-agent `env` defaults | generic handoff text; with only `exec_template`, generic recipes are built around it; absent env keys inherit the operator environment |
| `[transcript]` | `events_filename` + exactly one of `parser` (a named capability) or `extract` (the declarative tier) | `transcript_check` grades unverifiable; `command_check` and `llm_judge` carry grading; tokens/duration unrecorded |
| `[conversation]` | native `resume_exec_template` using the captured session id; optional token aggregation (`sum` default, `last` for cumulative reports) | no safe fallback: evals declaring `turns` are rejected in run preflight |
| `[model]` | `flag` | `--agent-model`/`--judge-model` recorded as provenance only |
| `[staging]` + `[skills_block]` | slug/naming rules, skills-block format | `--no-stage` inlining |
| `[tools]` | tool-name vocabulary by role | required alongside `[transcript]` (the stray-writes audit classifies by it) |
| `[shadow]` | `preflight` (named capability); optional `isolates_live_sources` assertion | no shadow report — correct for harnesses that load nothing global |
| `[guard]` | **built-ins only** — see below | `detect-stray-writes` audits after the fact |

### Named capabilities: real code for free

Everything that is genuinely code — transcript stitching, slug sanitization, shadow scanning —
is a **named capability** a descriptor references. If your harness emits a compatible stream,
you get the full feature from configuration alone:

- `transcript.parser = "claude-stream-json"` — Claude Code `-p --output-format stream-json` events.
  Also the only parser that surfaces **permission-denied tool calls** (from the terminal `result`
  event's `permission_denials`), which drives `permission-denials.json` and its validity warning.
- `transcript.parser = "codex-items"` — Codex `item.started`/`item.completed` JSONL.
- `transcript.parser = "opencode-events"` — OpenCode `run --format json` `tool_use`/`text`/`step_finish` events.
- `staging.slug_capability = "opencode"` — OpenCode's sanitizing slug rules.
- `shadow.preflight = "claude-plugins"` — the Claude plugin/global-skills shadow scan.
- `shadow.preflight = "codex-skills"` — the Codex repo/user/admin/plugin skill scan.
- `shadow.preflight = "opencode-skills"` — the OpenCode project/global `.opencode`/`.claude`/`.agents` skill scan.

`shadow.isolates_live_sources = true` is not a capability and does not change the scan. It is an
unverified operator assertion that every source the selected preflight can report is excluded from
every initial and resumed eval-agent dispatch. Detected sources remain in `plugin-shadow.json`,
which records the assertion, and `run` prints an informational notice instead of a warning;
`aggregate` then omits those findings from `validity_warnings`. Do not set it for partial isolation.
eval-magic deliberately does not inspect shell templates for known flags.

A built-in overlay may declare only the assertion and inherit the preflight capability:

```toml
label = "claude-code"

[shadow]
isolates_live_sources = true
```

A new harness still has to declare `preflight` in its fully resolved descriptor.

For example, a harness that logs Codex-compatible item JSONL gets full transcript ingest — parsed
tool invocations, `transcript_check` grading, the works — with:

```toml
[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "cool-events.jsonl"
parser = "codex-items"
```

An unknown capability name fails the schema gate listing the allowed values.

### The declarative extractor: flat streams with zero code

A stream where every tool event is **self-contained** — name, args, and result in the same
record — doesn't need a code parser at all: a `[transcript.extract]` block describes the mapping
and one generic engine executes it, unlocking the same transcript ingest a named parser gives
(`transcript_check` grading, parsed invocations in `run.json`, token/duration backfill into
`timing.json`). The built-in `codex` harness ingests through exactly this block:

```toml
[transcript]
events_filename = "codex-events.jsonl"
surfaces_skill_invocation = false

# Flat tool-item mapping: each matching record's item object becomes one
# tool invocation, in stream order.
[transcript.extract.tools]
where = { type = "item.completed" }
item = "item"       # dotted path to the tool object; omit when the record is the item
name_field = "type" # the item field naming the tool; records without it are skipped
skip_names = ["agent_message", "reasoning", "plan_update"]
args_omit = ["id", "type", "status", "output", "result", "error"]
result_coalesce = ["output", "result", "error"]

# Final text: dotted-path string pick over matching records; last match wins.
[transcript.extract.final_text]
where = { type = "item.completed", "item.type" = "agent_message" }
field = "item.text"

# Ordered assistant messages and the native session id are additionally
# required when [conversation] is declared.
[transcript.extract.assistant_messages]
where = { type = "item.completed", "item.type" = "agent_message" }
field = "item.text"

[transcript.extract.session_id]
where = { type = "thread.started" }
field = "thread_id"

# Tokens: over matching records, sum the required fields, subtract the optional
# fields, and clamp each record at zero.
[transcript.extract.tokens]
where = { type = "turn.completed" }
sum = ["usage.input_tokens", "usage.output_tokens"]
subtract = ["usage.cached_input_tokens"]

# Duration: last minus first RFC 3339 timestamp — or `field = "<path>"` to
# pick a millisecond value directly (declare exactly one of the two).
[transcript.extract.duration]
timestamp_spread = "timestamp"
```

The primitive set is deliberately minimal:

1. **`where`** — a dotted-path → expected-string equality filter shared by every sub-table
   (omitted = every record matches). String equality only: no operators, negation, or wildcards.
2. **`final_text`** — a field pick; the last matching record wins.
3. **`assistant_messages`** — every matching field pick, preserving stream order.
4. **`session_id`** — a field pick for the native resumable session id; the last match wins.
5. **`tools`** — the flat tool-item mapping: name from `name_field`, args = the item minus
   `args_omit` (key order preserved; `null` when nothing remains), result = the first *present*
   field of `result_coalesce` (present-but-null counts; strings verbatim, everything else compact
   JSON).
6. **`tokens`** — sum the required `sum` fields, subtract the optional `subtract` fields, and
   clamp each matching record's subtotal at 0. A missing or non-integer field counts 0, but a
   record where no `sum` path resolves leaves the total untouched — it never turns an absent
   total into zero.
7. **`duration`** — a millisecond `field` pick (last match wins) or `timestamp_spread` (last
   minus first; needs at least two parseable timestamps).

Dotted paths (`"usage.input_tokens"`, `"item.text"`) descend nested objects only — there is no
array indexing, and keys containing literal dots are unaddressable. Malformed JSONL lines are
silently skipped, as with every parser. `[transcript]` requires the `[tools]` write/shell
vocabulary alongside it (the stray-writes audit classifies by it). Leave
`surfaces_skill_invocation` false unless the mapping verifiably yields a deterministic
skill-invocation event; when true, the `__skill_invoked` meta-check matches the tool named by
`skill_tool` whose `skill_arg` argument equals the staged slug (defaults `"Skill"` / `"skill"` —
Claude Code's spellings; OpenCode declares `"skill"` / `"name"`).

**If a stream needs more than these primitives, it's a code capability, not a bigger DSL.** The
line is cross-event state: a stream whose tool results arrive in *separate* records joined by id
(Claude Code's `tool_use`/`tool_result` pairing), or whose content needs shape-dependent
coercion, is what named parsers are for — that's `claude-stream-json`, and it stays code.

### The guard restriction

User-supplied descriptors may **not** declare `[guard]` or set `run.supports_guard = true`: the
write guard **fails open** — a mistyped guard block would silently disarm it — so guard data
stays restricted to the embedded built-in descriptors. Unguarded runs fall back to the
`detect-stray-writes` audit: on a harness defined only by user descriptors the guard's auto-arm
quietly stays off (the `run` preflight warns naming the fallback), and only an explicit
`run --guard` is rejected in preflight (the run stops rather than continuing silently unguarded).
A project-local overlay of a guarded built-in is fine — the restriction applies to the user
file's own content, and the embedded guard merges through underneath it.

## Verify, don't guess

Every CLI flag, events filename, and event shape in a descriptor must come from the harness's own
documentation or from output you observed — never by analogy with another harness. Record each
value's source (docs URL, `--help` output, observed events file) plus the harness version in the
scaffolded notes file, `.eval-magic/harnesses/<name>-notes.md`: it is the verification record
behind the descriptor, and the same file an upstream PR promotes to `docs/<name>-notes.md`. The
`harness init` template embeds a VERIFY prompt at every field where guessing is tempting.

## The workflow

```sh
eval-magic harness init <name>   # scaffold the commented descriptor template + notes
                                 # skeleton — lint-clean before a single field is filled in
eval-magic harness lint <file>   # full per-file report: TOML + schema, user-layer
                                 # restrictions, cross-field invariants (merged onto the
                                 # registered harness with the same label, if any)
eval-magic harness lint <name>   # strictly re-lint every discovered layer of a name —
                                 # surfaces files that registry init skipped with a warning
eval-magic harness list          # every registered harness: layers + declared enhancements
eval-magic harness show <name>   # the resolved post-merge descriptor as authorable TOML
```

Then `run --harness <name>` and read the warnings: each one names the fallback carrying that part
of the run, which doubles as your wiring roadmap — declare the enhancement and the warning
disappears. Close the loop end-to-end before a real dispatch with
`eval-magic harness lint <name|file> --probe`: it renders `dispatch.exec_template` with a trivial
prompt and the resolved `[dispatch.env]` defaults in a throwaway temp dir, asks for confirmation,
runs the command via `/bin/sh -c` from that dir, and verifies `outputs/final-message.md` is
recovered (non-empty). It also
render-only-validates `parallel_command_template` / `judge_command_template` — rendering each
with stand-in values and reporting any unresolved `{token}` the run would later surface.

`--probe` invokes the **real** harness CLI (network, tokens, usage limits), so it is strictly
opt-in and never part of standard CI checks — it is a one-time BYOH check. The confirm banner
prints the rendered command and a `y/N` prompt; pass `--yes` to skip it (default-deny when stdin
is not a TTY, so the probe never spends usage unattended in a pipe), and `--probe-timeout <SECS>`
(default 300 = 5 min) to cap the run. Run it only after `harness lint` already passes the static
checks — a static failure aborts before any exec. See `harness lint --help` for the full contract.

## Upstreaming your descriptor

A descriptor that proves out locally can become a built-in — as a **data-only PR**, no Rust
beyond a mechanical registration. What counts as data vs code:

- **Data — one descriptor PR:** the descriptor file itself: `label` / `skills_dir` /
  `config_dirs`, the `[dispatch]` / `[conversation]` templates, `[model]`, `[staging]` + `[skills_block]`,
  `[tools]`, the `[run]` booleans, a `[transcript.extract]` block (the declarative tier is pure
  data), and `[transcript]` / `[shadow]` **when they reuse an existing named capability**
  (`claude-stream-json`, `codex-items`, `opencode-events`, `opencode`, `claude-plugins`,
  `codex-skills`, `opencode-skills`).
- **Code — one capability per PR, separate from the descriptor PR:** a new transcript parser,
  slug capability, or shadow preflight (each is `src/adapters/capabilities.rs` + a
  `src/adapters/<harness>/` module + a schema enum entry), and guard support (`[guard]` data is
  built-in-only — see the guard restriction above).

The PR itself is four files — open it with the
[harness-descriptor PR template](../.github/PULL_REQUEST_TEMPLATE/harness-descriptor.md), either
via the compare-URL query `?template=harness-descriptor.md&expand=1` or with
`gh pr create --body-file` on a filled copy:

1. `harnesses/<label>.toml` — your `.eval-magic/harnesses/<label>.toml`, promoted.
2. `src/adapters/descriptor.rs` — its `EMBEDDED_DESCRIPTORS` entry (mechanical registration).
3. `docs/<label>-notes.md` — your scaffolded notes file, promoted: the verification record for
   every field, per the don't-guess guardrail.
4. `README.md` — a new support-table row, baseline first, only the wired enhancement columns
   checked. Only upstreamed built-ins get a table row — project-local descriptors never do.

Attach the evidence the template asks for: clean `harness lint` output, a smoke eval run
end-to-end (`run` → dispatch → `ingest` → `finalize`), and the harness version you verified
against.
