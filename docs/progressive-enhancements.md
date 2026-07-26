# Harness progressive enhancements

> **Audience:** developers working on the eval-magic codebase. The README and `eval-magic --help`
> are the user-facing docs; this file explains how harness support is structured in code and what
> wiring more of it buys. Per-harness implementation notes live in [claude-notes.md](claude-notes.md),
> [codex-notes.md](codex-notes.md), and [opencode-notes.md](opencode-notes.md). Pointing eval-magic
> at a harness it doesn't know — user-supplied descriptor files, layering, `harness
> list`/`show`/`lint` — is the user-facing [byoh.md](byoh.md).

Harness compatibility is not a parity checklist to audit — it is **a minimal baseline every harness
satisfies, plus optional enhancements** a harness's adapter opts into. Most missing enhancements
have a documented lower-fidelity fallback. Native conversation resume is the deliberate exception:
an eval that declares scripted `turns` is rejected when the harness cannot preserve one session.

## One dispatch mechanism

One-shot evals and judges dispatch through the harness CLI, one subprocess per task. Scripted evals
use `eval-magic dispatch-task`, one subprocess per round while resuming one harness-native session
and one `eval_root`. The **generated artifacts are the runtime source of truth** for how to
dispatch: `run` writes `RUNBOOK.md` and `dispatch-manifest.md` carrying the exact per-task recipe
for the selected harness — hand-maintained docs never carry command recipes.

## The baseline contract

A harness qualifies at baseline with no harness-specific code beyond naming itself:

1. **A headless exec command** — some way to invoke the harness with a prompt from a chosen cwd and
   let it run to completion.
2. **A recoverable final message** — the agent writes `outputs/final-message.md` (the dispatch
   prompt asks for this), or transcript ingest recovers it where it is wired.
3. **`--no-stage` when native staging isn't wired** — each `SKILL.md` is inlined into its dispatch
   prompt instead of staged for native discovery.

That baseline already yields a working eval: `llm_judge` assertions grade soft behavior,
runner-owned `command_check` assertions can inject held-out files and execute deterministically,
runner-owned final-environment metrics land in `diff-scope.json`, `diff_scope` assertions gate
files/lines deterministically, and the `detect-stray-writes` post-pass (folded into `ingest`) audits
writes that leave the private task environment. Run records without transcript ingest are assembled
from `outputs/final-message.md` or by hand per `schema/run-record.schema.json`.

In descriptor terms the baseline is one required field: `label`. Everything else in a harness
descriptor is optional for one-shot evals — an absent field or table gets a working generic fallback, and the `run`
preflight *warns* naming that fallback rather than rejecting (a harness without `skills_dir`
forces `--no-stage`; without a declared guard the run continues unguarded behind the
`detect-stray-writes` audit; requested models without a model flag are recorded as provenance
only). Supported enhancements are provided automatically — the write guard auto-arms wherever a
harness declares one and staging is active (`--no-guard` opts out). Only genuinely contradictory
flag combinations stay errors. A selected eval with `turns` also requires `[conversation]`; no
generic fresh-session fallback can preserve the meaning of a canned reply.

## Where this lives in code

- `harnesses/<label>.toml` — one embedded **descriptor file** per built-in harness, carrying every
  declarative value (labels, dirs, capability booleans, phrases, command templates, banner prose).
  Descriptors are schema-gated (`schema/harness-descriptor.schema.json`) and invariant-checked at
  load time.
- `src/adapters/registry.rs` — the label-keyed registry: `init_registry` loads every **layer**
  (embedded → user-global → project-local → `--harness-file`) with field-level merge per label,
  and `adapter_for()` resolves each `Harness` handle to its descriptor-backed adapter — still the
  single dispatch point. Broken discovered files skip with a warning; the layer model itself is
  documented in [byoh.md](byoh.md).
- `src/adapters/descriptor/layers.rs` — layer discovery (config-root resolution, per-directory
  scans, the `--harness-file` top layer) and the user-layer restrictions (no `[guard]`).
- `src/adapters/harness.rs` — the `HarnessAdapter` trait, tiered into baseline and enhancement
  sections.
- `src/adapters/descriptor.rs` — the descriptor model, the per-file schema gate
  (`parse_descriptor_value`), field-level merge (`merge_descriptor_value`), and
  `finalize_descriptor` (the load-time invariants, with merged-file provenance in every message);
  `src/adapters/descriptor_adapter.rs` — the one generic `DescriptorAdapter` implementing the
  trait from a descriptor.
- `src/adapters/capabilities.rs` — the **named capabilities**: closed enums a descriptor references
  by kebab-case name for everything that is real code (transcript parsers, slug generation,
  shadow preflight). The write guard needs no named capability: it is pure `[guard]` data
  rendered by the one generic engine in `src/adapters/guard.rs`.
- `src/adapters/<harness>/` — only the code behind those capabilities: transcript parsers,
  harness-native shadow scans, the OpenCode slug sanitizer.
- `run_capabilities()` (descriptor table `[run]`) + `harness_run_preflight()`
  (`src/cli/run/util.rs`) — the `run` preflight: it resolves the guard tri-state (auto-arm when
  the harness declares a guard and staging is active; `--guard`/`--no-guard` make it explicit),
  and undeclared enhancements warn naming their fallback and adjust the options (guard forced
  off, missing `skills_dir` forces `--no-stage`, the no-transcript-parser warning scoped to eval
  configs that actually use `transcript_check`, missing dispatch recipes noted); only
  contradictory flag combinations (`--bootstrap`/`--stage-name` where the descriptor declares
  them incompatible with `--no-stage`) and an explicit `--guard` on a user-descriptor-only
  harness reject.

## Runner-owned environment checks are baseline

Every canonical `(eval, condition, run)` gets a distinct `eval_root`. After fixtures, staging, and
guard installation, `run` recreates a runner-owned Git repository at that root, commits the task
state on branch `work`, runs shadow preflight at the resulting repository boundary, and snapshots
the task environment. Git is therefore a runtime prerequisite; each task starts clean and has no
remotes. During `ingest`, before any held-out setup is injected, the runner compares that baseline
with the final environment and writes raw `files_touched`, `lines_added`, `lines_removed`, and
zero-context Myers `hunks` to `diff-scope.json`. Framework artifacts under the task root's
`.eval-magic-outputs/` and runner-owned `.git/` are excluded; nested repository metadata and all
other new files count. `benchmark.json` preserves these metrics per run even without a `diff_scope`
assertion. An assertion may gate `max_files_touched`, `max_lines_changed` (added plus removed), or
both.

This is deliberately a secondary signal: a smaller diff can be focused, but it can also be
incomplete. Pair a scope gate with a correctness assertion.

`command_check` is intentionally not a harness enhancement. `run` detects the assertion before
dispatch so it can validate held-out sources before building. After diff-scope capture, `ingest`
copies the assertion's held-out `setup_files` from the skill's `evals/` directory into that root and
executes the trusted command through the platform shell. Root `.git` paths are reserved for both
visible and held-out fixtures, while nested repositories remain valid. The runner clears inherited
Git routing variables before optional `env` values override the environment; optional `matrix`
values execute every Cartesian-product cell and persist per-cell results. The files are never staged
or mentioned to the agent, and therefore never inflate scope metrics.

This path needs no transcript parser, tool vocabulary, model flag, or judge recipe, so it behaves
the same for built-ins and descriptor-only harnesses. It also does not use harness tools: an armed
agent write guard can remain installed while the runner executes the command. `finalize` converts
the schema-gated intermediate result into an ordinary grading result, leaving aggregation
harness-agnostic.

## The enhancements

Each enhancement is a group of descriptor fields — plus a named capability where code is involved —
with defaults. Wire them together; load-time descriptor validation (`validate_descriptor` in
`src/adapters/descriptor.rs`) rejects the combinations that must move in lockstep, with an
actionable message per violation.

### Transcript ingest

*Why harness-specific:* every harness persists a different event stream (Claude Code's `-p`
stream-json, Codex's `item.completed` JSONL) — but not all of them need per-harness code. Ingest
is two-tier: a **flat** stream (each tool event self-contained) is a declarative mapping the
generic extract engine (`src/adapters/extract.rs`) executes from descriptor data alone, while a
stream needing cross-event state — keyed `tool_use`/`tool_result` joins, shape-dependent content
coercion — is real per-harness code behind a named capability. If a stream needs more than the
extract primitives, it's a code capability, not a bigger DSL.

*What it unlocks:* `transcript_check` assertions, token/duration capture, automatic
`run.json`/`timing.json` assembly by `ingest`, and — where the transcript exposes a skill-tool
event — a deterministic `__skill_invoked` meta-check.

*Fallback:* `transcript_check` grades as *unverifiable*, `llm_judge` and runner-owned
`command_check` carry the grading (bias suites toward those for such a harness), tokens/duration go
unrecorded, records are assembled from `outputs/final-message.md` or by hand, and the meta-check
uses the LLM-judge fallback.

*Descriptor fields:* the `[transcript]` table — `events_filename` (gate: an absent table means the
ingest pipeline never calls a parser), exactly one of `parser`/`extract` (validation rejects both
or neither), and `surfaces_skill_invocation`. The `extract` sub-table is the declarative tier:
equality `where` filters, final and ordered assistant-text picks, a session-id pick, flat
tool-item mapping, token sum, and duration rule, documented with a worked example in
[byoh.md](byoh.md) — the built-in `codex` descriptor ingests through it.
*Capability:* `transcript.parser` names the code that stitches a non-flat stream
(`claude-stream-json`, `opencode-events`; `codex-items` is the reference implementation the
extract engine's differential test compares against) — a new harness emitting a compatible event
stream reuses one with zero code.
The tool names the transcript yields must be declared in `[tools]` (see the write-guard
enhancement) or `detect-stray-writes` audits nothing for the harness — validation rejects the
combination.

### Native conversation resume

*Why harness-specific:* each CLI spells same-session continuation differently and exposes its
session identifier in a different transcript event.

*What it unlocks:* an eval's ordered `turns` array. `dispatch-task` starts the normal one-shot
command, extracts the native session id, evaluates `agent_asks` (`?`) plus the optional response
regex, and resumes the same session for each delivered follow-up. It writes raw round transcripts
under `outputs/turn-N/` and atomically commits `conversation.json` only after a complete or normal
guardrail-stopped scenario. `ingest` skips an interrupted task with no completion artifact.

*Fallback:* none. `run` rejects selected multi-turn evals when the harness omits this capability;
silently starting a fresh session would make the canned user response meaningless.

*Descriptor fields:* `[conversation].resume_exec_template`, with required
`<eval-root>`, `<outputs_dir>`, `{session_arg}`, and `{prompt_arg}` placeholders. It requires
`dispatch.exec_template` and transcript parsing. Declarative transcript extraction must include
`assistant_messages` and `session_id`; named built-in parsers provide the same normalized fields.

### Native skill staging + skills block

*Why harness-specific:* each harness has its own project-local discovery dir and its own way of
surfacing discoverable skills to a session (Claude Code's Skill-tool list, Codex's `## Skills`
markdown, OpenCode's `<available_skills>` XML), and some constrain skill naming.

*What it unlocks:* environment parity — the staged skill is discovered the way a real install
would discover it, instead of being pasted into the prompt.

*Fallback:* `--no-stage` inlines each `SKILL.md` into its dispatch prompt.

*Descriptor fields:* `skills_dir`, `config_dirs`, and the `[staging]` table — `slug_template`
(or `slug_capability`), `stage_name_pattern`/`stage_name_max_len`/`stage_name_invalid_message`
(declarative naming rules), `rewrites_frontmatter_name`, `advertises_staged_slug_name`,
`surface_phrase`, `unresolved_phrase` — plus the `[skills_block]` table (`header`/`item`/`footer`
format strings with `{name}`/`{description}`/`{path}` placeholders, rendered by the shared
`skills_block` renderer).
*Capability:* `staging.slug_capability` for naming rules that need sanitization/truncation beyond
a format string (`opencode`); simpler rules stay declarative.

### Model flag

*Why harness-specific:* the CLI flag (and its position in the command) differs per harness.

*What it unlocks:* `--agent-model` / `--judge-model` actually select models in the generated
recipes; judge tasks resolve a per-task model.

*Fallback:* the models are recorded as provenance in `conditions.json` only; dispatches run on the
harness's default model.

*Descriptor fields:* the `[model]` table — `flag` (consumed as `{model_arg}` by the dispatch
templates).

### Write guard

*Why harness-specific:* the guard arms a *native pre-tool hook* — hook surface, blocking
mechanism, trust model, and deny-verdict shape are all harness-native (Claude Code's
`settings.local.json` + `hookSpecificOutput`, Codex's `hooks.json` + `{"decision": "block"}`,
OpenCode's auto-loaded project plugin that blocks by throwing the reason).

*What it unlocks:* out-of-bounds writes are *blocked before they happen* instead of detected
afterwards. The guard is provided automatically: every staged run of a guard-declaring built-in
arms it unless `--no-guard` opts out (`--guard` makes the request explicit, turning silent
can't-arm cases into a warning or error). Every block is also recorded as privacy-safe metadata
under the task env and collected into `guard-denials.json`; `aggregate` emits one validity warning
per affected task because even an intentional boundary block changed agent behavior.

*Fallback:* `detect-stray-writes` audits after the fact. (It also flags **live-source reads** — an
arm whose subagent read the live skill source instead of its staged copy, which contaminates the
arm; fatal in revision mode, where the `old_skill` arm then sees new-skill content.)

*Descriptor fields:* the `[guard]` table — `verdict_template` and `armed_message` for every
engine, `engine` (the install-mechanism discriminator, default `json-hooks`), and the per-engine
fields: `json-hooks` (Claude Code, Codex) declares `hooks_file`, `matcher`, `command_template`,
and `hook_entry`; `opencode-plugin` declares `plugin_file` instead — plus `[tools]` (the
write/patch/shell/read vocabulary) and `run.supports_guard` (validated to stay in lockstep with
the `[guard]` table). There is no guard code capability: one engine module
(`src/adapters/guard.rs`) holds two install arms selected by `engine` and a single shared verdict
path. `json-hooks` merges the rendered hook entry into `hooks_file`; `opencode-plugin` stages an
embedded JS project plugin (`harnesses/opencode-guard-plugin.js`, `{exe}`/`{marker}` substituted
as JSON string literals) whose `tool.execute.before` hook forwards every tool call to the generic
entry point and throws the verdict's reason to block — OpenCode auto-loads project plugins by
directory convention, so no dispatch flag is needed. The templates' authored JSON key order is
serialized verbatim — the verdict bytes are the harness's on-disk contract. Validation proves the
per-engine shape (the schema's conditional requiredness, plus load-time checks barring the other
engine's fields), that every hooked `matcher` tool is declared in `[tools]` (json-hooks), that
the templates parse as JSON, and that their `{command}`/`{matcher}`/`{reason}` placeholders sit
in string values. Patch payload extraction accepts structured `files` plus raw
`command`/`patch`/`input`/`content`/`patchText` bodies and validates every source and destination.
The quote-aware Bash scanner resolves literal redirect and `tee` targets from the invocation cwd;
dynamic, malformed, or outside targets remain denied. It allows local Git inspection, staging,
commits, and branching within the task boundary, but denies outside/dynamic repository routing,
`git worktree add`, remote-capable subcommands, mutating `git remote`, and remote/url config writes.
Read-only `git remote`, `git remote -v`, `git remote get-url`, and config reads remain available.
`--no-guard` opts out of these blocks, though the task repository still begins without remotes. The
guard arbiter and
`detect-stray-writes` classify tool names against the cross-harness vocabulary union
(`all_tool_vocabulary`), so wiring a guard or transcript ingest without declaring the harness's
tool names is rejected at descriptor load. The hidden `guard` /
`guard-codex` subcommands are frozen hook entry-point aliases (a stable on-disk contract); a new
guard-capable built-in uses the generic `guard-hook --harness <label>` entry point (OpenCode's
plugin spawns exactly that) — a descriptor `[guard]` block is all it takes, no bespoke
install/verdict code beyond the engine arms. Shared marker/manifest/teardown machinery lives in
`src/sandbox/`. **User-supplied descriptors may not declare `[guard]`** — the guard fails open,
so a mistyped user guard block would silently disarm it. On such a harness auto-arm quietly
stays off (the preflight warns naming the `detect-stray-writes` fallback); only an *explicit*
`--guard` is rejected in preflight, since a run the user asked to guard must not continue
silently unguarded.

### Shadow preflight

*Why harness-specific:* what "discoverable from the live environment" means is harness-native.
Claude Code loads enabled plugins and its global skills dir. Codex loads repository-ancestor,
user, and admin skill directories plus enabled installed plugins. OpenCode loads project and
global `.opencode`, `.claude`, and `.agents` skill dirs — including skills installed for other
harnesses. A logical eval skill present in any such source contaminates the with/without
comparison even when the staged copy uses a unique slug.

*What it unlocks:* a build-time contamination warning (banner + `plugin-shadow.json` in the
iteration dir), which `aggregate` folds into `benchmark.json` validity warnings.

*Fallback:* no preflight — the run proceeds with no shadow report. This does not prove the live
environment is clean; the operator must check any harness-native global discovery sources.

*Descriptor fields:* the `[shadow]` table — `preflight`.
*Capability:* `shadow.preflight` names the scan (`claude-plugins`, `codex-skills`, or
`opencode-skills`). It returns the harness-neutral `PluginShadowReport` from
`src/adapters/skill_shadow.rs`; detection and remediation rendering stay in the harness's
module tree. Both scans are best-effort: Codex's does not enumerate bundled system skills (no
stable listing exists), and OpenCode's does not scan config-declared `skills.paths`/`skills.urls`
sources.

### Plan-mode context

*Why harness-specific in principle:* a harness could inject a real native plan mode.

*What the default does:* wraps the shared `profiles/shared/plan-mode.md` procedure in a
`<system-reminder>` block — an approximation that is the same for every harness today, since plan
modes can't be reproduced exactly in a one-shot dispatch anyway.

*Descriptor fields:* none yet — this is the trait default (`render_plan_mode_context`) with no
descriptor surface; a harness with a real native plan mode would grow one.

### Dispatch recipes

*Why harness-specific:* the copy-pasteable command template is the harness's CLI.

*What it unlocks:* `RUNBOOK.md`, `dispatch-manifest.md`, and the post-`run`/post-`ingest` handoffs
carry exact per-task commands (including parallel and judge variants).

*Fallback:* the generic handoff text; the operator constructs dispatch commands themselves. The
`run` preflight warns naming this limitation when the descriptor declares no `exec_template`
(`has_dispatch_recipes()`).

*Descriptor fields:* the `[dispatch]` table — `exec_template`, `parallel_command_template`,
`judge_command_template`, `next_steps_template`, `manifest_template`, `capture_prefix`,
`guard_args`, `model_note`. Templates carry `{model_arg}`/`{guard_args}` slots the renderer fills
for eval-agent dispatches; judge commands deliberately render empty `guard_args` because they run
outside the guarded task envs. The shared jq/xargs parallel and judge scaffolds stay code
(`src/adapters/cli_command.rs`) with the per-harness command block spliced in. Validation rejects
a template whose placeholder has no backing field.

## Current support

The **Harnesses table in the README is the source of truth** for which harness has which
enhancement — keep it in sync with the adapters when wiring or dropping one.

## Adding a new harness

1. **Start as a user descriptor** — scaffold it with `eval-magic harness init <label>` (a
   commented template plus a notes skeleton, lint-clean as written), fill in verified values per
   [byoh.md](byoh.md), and iterate with `harness lint`/`show` and real runs. No Rust, no rebuild;
   this is also where the descriptor's field set gets proven.
2. **Promote to a built-in** once it earns bundling: move the file to `harnesses/<label>.toml` and
   add it to `EMBEDDED_DESCRIPTORS` (`src/adapters/descriptor.rs`). The registry keys on the
   descriptor's `label`. This is the data-only descriptor-contribution PR described in byoh.md's
   "Upstreaming your descriptor" — open it with
   `.github/PULL_REQUEST_TEMPLATE/harness-descriptor.md`.
3. Create `docs/<harness>-notes.md` with the implementation notes discovered along the way —
   promoted from the scaffolded `.eval-magic/harnesses/<label>-notes.md`. The PR template
   requires it: the notes file is where the don't-guess guardrail's verification evidence lives.
4. Add the harness to the README support table (all enhancements ❌ at baseline).
5. Wire enhancements in leverage order — dispatch recipes and transcript ingest first (they carry
   the most fidelity and are prerequisites for conversation resume), then conversation resume,
   staging, model flag, guard (guard requires built-in status — user
   descriptors may not declare one) — updating the table as each lands. Most enhancements are
   descriptor fields; add a named capability in `src/adapters/capabilities.rs` (plus its
   `src/adapters/<harness>/` module) only when the harness's stream or hooks are incompatible with
   every existing capability.

## Guardrails

- **Cross-harness compatibility is enforced.** A change for one harness must not regress another;
  load-time descriptor validation (`src/adapters/descriptor.rs`), the golden-artifact fixtures
  under `tests/golden/` (re-bless deliberate output changes with `GOLDEN_BLESS=1 cargo test
  golden_`), the per-value pins in `src/adapters/harness.rs`, and the per-harness integration
  tests under `tests/run/` are the floor.
- **One enhancement per PR.** Wiring a harness happens one capability at a time.
- **Don't guess harness details.** CLI flags, hook shapes, and event vocabularies come from the
  harness's own documentation or observed output — record what you verified in the harness's notes
  file. The `harness init` scaffold embeds these prompts inline at every field, and the
  harness-descriptor PR template requires the per-field source attestation.
