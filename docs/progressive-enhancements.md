# Harness progressive enhancements

> **Audience:** developers working on the eval-magic codebase. The README and `eval-magic --help`
> are the user-facing docs; this file explains how harness support is structured in code and what
> wiring more of it buys. Per-harness implementation notes live in [claude-notes.md](claude-notes.md),
> [codex-notes.md](codex-notes.md), and [opencode-notes.md](opencode-notes.md).

Harness compatibility is not a parity checklist to audit — it is **a minimal baseline every harness
satisfies, plus optional enhancements** a harness's adapter opts into. A missing enhancement
degrades fidelity, never correctness: every enhancement has a documented fallback.

## One dispatch mechanism

Every eval test and judge is dispatched the same way: through the harness's one-shot CLI, one
subprocess per task, each `cd`'d into its `(group, condition)` env and writing its output to disk.
There is no other mode. The **generated artifacts are the runtime source of truth** for how to
dispatch: `run` writes `RUNBOOK.md` and `dispatch-manifest.md` carrying the exact per-task recipe
for the selected harness (rendered by the adapter's dispatch-recipe methods) — hand-maintained docs
never carry command recipes.

## The baseline contract

A harness qualifies at baseline with no harness-specific code beyond naming itself:

1. **A headless exec command** — some way to invoke the harness with a prompt from a chosen cwd and
   let it run to completion.
2. **A recoverable final message** — the agent writes `outputs/final-message.md` (the dispatch
   prompt asks for this), or the transcript parser recovers it where one exists.
3. **`--no-stage` when native staging isn't wired** — each `SKILL.md` is inlined into its dispatch
   prompt instead of staged for native discovery.

That baseline already yields a working eval: `llm_judge` assertions grade every behavior, and the
`detect-stray-writes` post-pass (folded into `ingest`) audits out-of-bounds writes from whatever
run records exist. Run records without a transcript parser are assembled by hand per
`schema/run-record.schema.json`.

In trait terms the baseline is two methods: `label()` and `skills_dir()`. Everything else on
`HarnessAdapter` has a default — either a working generic fallback or an `Unsupported` error naming
the enhancement it belongs to.

## Where this lives in code

- `src/adapters/harness.rs` — the `HarnessAdapter` trait, tiered into baseline and enhancement
  sections; `adapter_for()` is the single place a concrete harness is named.
- `src/adapters/<harness>/` — everything specific to one harness: the adapter impl, session
  renderers, transcript parsers, dispatch-recipe rendering, guard hooks.
- `run_capabilities()` — the narrow table the `run` preflight uses to accept or reject run options
  (`--guard`, `--bootstrap`/`--stage-name` with `--no-stage`) per harness.

## The enhancements

Each enhancement is a group of trait methods with defaults. Wire them together; invariant tests in
`harness.rs` catch the combinations that must move in lockstep.

### Transcript parser

*Why harness-specific:* every harness persists a different event stream (Claude Code's `-p`
stream-json, Codex's `item.completed` JSONL) — parsing is real per-harness code, not a mapping.

*What it unlocks:* `transcript_check` assertions, token/cost/duration capture, automatic
`run.json`/`timing.json` assembly by `ingest`, and — where the transcript exposes a skill-tool
event — a deterministic `__skill_invoked` meta-check.

*Fallback:* `transcript_check` grades as *unverifiable*, `llm_judge` carries the grading (bias
suites toward `llm_judge` for such a harness), tokens/duration go unrecorded, records are
hand-assembled, and the meta-check uses the LLM-judge fallback.

*Trait methods:* `cli_events_filename` (gate: `None` means the ingest pipeline never calls the
parsers), `parse_cli_events`, `parse_cli_events_full`, `transcript_surfaces_skill_invocation`.
The tool names the parser emits must be declared in `tool_vocabulary` (see the write-guard
enhancement) or `detect-stray-writes` audits nothing for the harness.

### Native skill staging + skills block

*Why harness-specific:* each harness has its own project-local discovery dir and its own way of
surfacing discoverable skills to a session (Claude Code's Skill-tool list, Codex's `## Skills`
markdown, OpenCode's `<available_skills>` XML), and some constrain skill naming.

*What it unlocks:* environment parity — the staged skill is discovered the way a real install
would discover it, instead of being pasted into the prompt.

*Fallback:* `--no-stage` inlines each `SKILL.md` into its dispatch prompt.

*Trait methods:* `skills_dir` semantics, `staged_slug`, `validate_stage_name`,
`rewrites_frontmatter_name`, `advertises_staged_slug_name`, `render_available_skills_block`,
`skill_surface_phrase`, `skill_unresolved_phrase`, `config_dir_names`.

### Model flag

*Why harness-specific:* the CLI flag (and its position in the command) differs per harness.

*What it unlocks:* `--agent-model` / `--judge-model` actually select models in the generated
recipes; judge tasks resolve a per-task model.

*Fallback:* the models are recorded as provenance in `conditions.json` only; dispatches run on the
harness's default model.

*Trait methods:* `cli_model_flag` (consumed by the harness's recipe renderers).

### Write guard

*Why harness-specific:* the guard arms a *native pre-tool hook* — hook config location, matcher
syntax, trust model, and deny-verdict shape are all harness-native (Claude Code's
`settings.local.json` + `hookSpecificOutput`, Codex's `hooks.json` + `{"decision": "block"}`).

*What it unlocks:* out-of-bounds writes are *blocked before they happen* instead of detected
afterwards.

*Fallback:* `detect-stray-writes` audits after the fact. (It also flags **live-source reads** — an
arm whose subagent read the live skill source instead of its staged copy, which contaminates the
arm; fatal in revision mode, where the `old_skill` arm then sees new-skill content.)

*Trait methods:* `install_guard`, `guard_armed_message`, `guard_hook_cleanup_dir`,
`tool_vocabulary`, plus `run_capabilities().supports_guard` (invariant-tested to stay in lockstep
with the banner). The guard arbiter and `detect-stray-writes` classify tool names against the
cross-harness vocabulary union (`all_tool_vocabulary`), so wiring a guard or transcript parser
without declaring the harness's tool names trips the invariant tests in `harness.rs`. The hidden
`guard` / `guard-codex` subcommands are the hook entry points — their names are a stable on-disk
contract. Shared marker/manifest/teardown machinery lives in `src/sandbox/`.

### Shadow preflight

*Why harness-specific:* what "discoverable from the live environment" means is harness-native —
Claude Code dispatches load the operator's enabled plugins and global skills dir, so a staged
skill name colliding with one of those contaminates the with/without comparison. Other harnesses
load nothing global today.

*What it unlocks:* a build-time contamination warning (banner + `plugin-shadow.json` in the
iteration dir), which `aggregate` folds into `benchmark.json` validity warnings.

*Fallback:* no preflight — the run proceeds with no shadow report, exactly right for a harness
whose dispatches load nothing beyond the staged env.

*Trait methods:* `detect_shadowed_skills` (returns the harness-neutral `PluginShadowReport` from
`src/adapters/skill_shadow.rs`; detection itself stays in the harness's module tree).

### Plan-mode context

*Why harness-specific in principle:* a harness could inject a real native plan mode.

*What the default does:* wraps the shared `profiles/shared/plan-mode.md` procedure in a
`<system-reminder>` block — an approximation that is the same for every harness today, since plan
modes can't be reproduced exactly in a one-shot dispatch anyway.

*Trait methods:* `render_plan_mode_context`.

### Dispatch recipes

*Why harness-specific:* the copy-pasteable command template is the harness's CLI.

*What it unlocks:* `RUNBOOK.md`, `dispatch-manifest.md`, and the post-`run`/post-`ingest` handoffs
carry exact per-task commands (including parallel and judge variants).

*Fallback:* the generic handoff text; the operator constructs dispatch commands themselves.

*Trait methods:* `cli_next_steps`, `cli_manifest_section`, `cli_judge_next_steps`.

## Current support

The **Harnesses table in the README is the source of truth** for which harness has which
enhancement — keep it in sync with the adapters when wiring or dropping one.

## Adding a new harness

1. Add the variant to `Harness` in `src/core/context.rs` (it derives `clap::ValueEnum`).
2. Create `src/adapters/<harness>/mod.rs` with the adapter struct implementing `label()` +
   `skills_dir()`, and register it in `adapter_for()` (`src/adapters/harness.rs`).
3. Create `docs/<harness>-notes.md` with the implementation notes discovered along the way.
4. Add the harness to the README support table (all enhancements ❌ at baseline).
5. Wire enhancements in leverage order — dispatch recipes and transcript parser first (they carry
   the most fidelity), then staging, model flag, guard — updating the table as each lands.

## Guardrails

- **Cross-harness compatibility is enforced.** A change for one harness must not regress another;
  the cross-harness tests in `src/adapters/harness.rs` and the per-harness integration tests under
  `tests/run/` are the floor.
- **One enhancement per PR.** Wiring a harness happens one capability at a time.
- **Don't guess harness details.** CLI flags, hook shapes, and event vocabularies come from the
  harness's own documentation or observed output — record what you verified in the harness's notes
  file.
