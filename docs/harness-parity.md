# Eval-Magic Harness Parity Check

You are an agent running inside one of the eval runner's supported harnesses. This file walks you through auditing **how completely your harness is wired into the eval runner** and prepping to close one gap.

This file covers the **`eval-magic` runner only** — the infrastructure in `eval-magic` that dispatches, records, and grades skill evals.

Read the file end-to-end before acting. Steps 4a and 4b are the source of truth for what "eval-magic parity" means today — when the runner gains a new dispatch mechanism or the `HarnessAdapter` trait gains a method, those steps are updated and this file stays evergreen.

## The two parity axes

Parity is organized around **run mode** — *how* an eval is dispatched — as the primary axis, with the harness adapter as the plug-in surface beneath it:

1. **Run-mode / dispatch-mechanism parity (primary).** Every dispatch uses one of two mechanisms, defined as `DispatchMechanism` in `src/core/run_mode.rs`:
   - **`InSession`** — the runner hands tasks to in-session subagents (Claude Code's Task tool). **Claude Code** is the reference.
   - **`Cli`** — each task is dispatched through a one-shot harness CLI subprocess (`codex exec`). **Codex** is the reference.

   These two mechanisms underpin the three *user-facing* run modes in the README's [Run modes](../README.md#run-modes) section: **fully-interactive** rides on `InSession`; **headless** and **hybrid** both ride on `Cli`. `mechanism_for(harness)` (same file) maps each harness to the mechanism it wires today — that mapping is the single place the harness↔mechanism coupling lives.

2. **Harness-adapter feature parity (the plug-in surface).** Each harness plugs into the runner through one impl of the **`HarnessAdapter`** trait in `src/adapters/harness.rs`, resolved by `adapter_for(harness)`. The trait's methods *are* the feature surface: skill-list rendering, transcript parsing, staged-skills dir, plan-mode profile, the write-guard hook, and the `Cli`-mechanism dispatch guidance. A harness reaches parity for a mechanism by implementing the trait methods that mechanism consumes.

A harness can wire a mechanism yet leave some adapter methods at their stub/default (lower fidelity), or implement rich adapter methods while only supporting one mechanism. Step 4 audits both axes.

---

## Step 1 — Identify your harness

Name the harness you are running in. You almost certainly already know — confirm by checking:

- Your invocation context and working directory
- The tool names available to you in this session
- Any session-start context block injected at the top of the conversation

The intended supported harnesses are: **Claude Code, Codex CLI, OpenCode**.

If the harness you are running in is not in that list, stop and ask the user before continuing.

---

## Step 2 — Read the reference materials

Read these in order. Paths are relative to the repository root.

| Source | What to look for |
|--------|------------------|
| `src/core/run_mode.rs` | `DispatchMechanism` and `mechanism_for`. The two dispatch mechanisms and which one your harness maps to today |
| `src/adapters/harness.rs` | The `HarnessAdapter` trait (the feature surface), the three impls, and `adapter_for`. The reference impls are `ClaudeCodeAdapter` (`InSession`) and `CodexAdapter` (`Cli`) — read the one that matches your mechanism |
| `src/adapters/claude_code_transcript.rs` and `src/adapters/codex_transcript.rs` | The reference transcript parsers (`parse_transcript*` / `parse_codex_events*`) that the trait's `parse_transcript` / `parse_transcript_full` delegate to. A second harness translates its transcript shape into the same `ToolInvocation` list / `TranscriptSummary` |
| `eval-magic --help` and the README's [Environment parity](../README.md#environment-parity) / [Harnesses](../README.md#harnesses) sections | The cross-harness breadcrumbs and the flag-by-flag reference. Treat the breadcrumbs as starting points, not specifications |

Do not skim. The parity report you produce in Step 4 is only as good as the reference you internalized here.

---

## Step 3 — Discover your harness's existing surface area

Enumerate, using ordinary file search, what already exists for your harness. Do not rely on memory — search the working tree. Useful heuristics:

- Your harness's arm in `adapter_for` and `mechanism_for`, and its `HarnessAdapter` impl in `src/adapters/harness.rs`
- The harness name anywhere inside `src/` (especially `src/adapters/`, `src/core/context.rs`) and `profiles/`
- A per-harness section in the README, or tests exercising the runner for the harness (`tests/`, e.g. `tests/run/codex.rs`, `tests/run/opencode.rs`)

Record every path you find. You will reference them in Step 4.

---

## Step 4a — Audit run-mode / mechanism parity

State which `DispatchMechanism`(s) your harness supports today, per `mechanism_for`, and whether the mechanism's path is end-to-end:

- **`InSession`** consumes: a subagents-dir transcript source (no `cli_events_filename`), and the in-session next-steps guidance (mechanism-level, not adapter-supplied).
- **`Cli`** consumes these `HarnessAdapter` methods: `cli_events_filename` (the per-task transcript file the CLI writes), `cli_model_flag` (the harness-native model-selection flag, when supported), `cli_next_steps` (the post-`run` dispatch guidance), `cli_manifest_section` (the dispatch-manifest recipe), and `cli_judge_next_steps` (the post-`grade` / post-`ingest` judge dispatch recipe).

A harness reaches mechanism parity when its mechanism's path runs end-to-end: dispatch guidance is emitted, the transcript is found and parsed, and `record-runs` / `fill-transcripts` assemble records. The intended end state is each harness supporting every mechanism that translates to it (the README's [Run modes](../README.md#run-modes) matrix tracks current support).

## Step 4b — Audit harness-adapter feature parity

For each `HarnessAdapter` method below, compare your harness's impl against the reference. Methods are described by what they *do* so they survive renames; when the trait changes, this list is updated and the rest of the file still applies.

| Adapter capability | Trait method(s) | Reference behavior |
|--------------------|-----------------|--------------------|
| Realistic eval environment (skill staging) | `skills_dir`, `render_available_skills_block`, `rewrites_frontmatter_name`, `advertises_staged_slug_name`, `skill_surface_phrase`, `skill_unresolved_phrase` | Stage skills under the harness's project-local dir and render the discoverable-skills block in the harness's **native** presentation, so a dispatch reads like a real session in that harness, not an eval. Claude Code: `.claude/skills/` + `The following skills are available for use with the Skill tool:`. The `--bootstrap` `<session-start-context>` wrapper and the slug-disambiguation framing are shared in `src/cli/run/dispatch.rs` |
| Skill-eval transcript adapter | `parse_transcript`, `parse_transcript_full` | Translate the harness's persisted transcript into the same `ToolInvocation` list and `TranscriptSummary` (final message, tool invocations, deduped usage/timing) every downstream stage consumes |
| Skill-eval auto-record (run/timing assembly) | (consumes `parse_transcript_full` + `cli_events_filename`) | `src/pipeline/record_runs.rs` assembles each task's `run.json` + `timing.json` from disk: carry-over fields from `dispatch.json`, `final_message` from `outputs/final-message.md`, and tool invocations/tokens/duration from the parsed transcript. A harness closes this gap by supplying the transcript its `parse_transcript_full` consumes (the portable fallback — hand-authored records against `run-record.schema.json` — is unchanged) |
| Cli model selection | `cli_model_flag`, `cli_next_steps`, `cli_manifest_section`, `cli_judge_next_steps` | For one-shot CLI dispatch, `run --agent-model` is rendered into the agent command recipe and `run --judge-model` becomes the default model in `judge-tasks.json`; assertion-level `llm_judge.model` remains the most specific override. Codex is the reference: `cli_model_flag` returns `-m`, agent recipes render `codex exec -m <model>`, and judge recipes read each task's resolved `model` and pass `-m "$model"` only when present |
| Eval subagent write enforcement | `install_guard` | Opt-in `--guard` stages a pre-tool hook (`src/sandbox/`) that *denies* subagent writes/installs outside the eval sandbox while dispatches run. Portable fallback for every harness: the `eval-magic detect-stray-writes` post-pass (`src/pipeline/detect_stray_writes.rs`) flags out-of-bounds writes from the parsed transcript after the fact |
| Eval plan-mode operating context | `plan_mode_profile`, `render_plan_mode_context` | Opt-in `--plan-mode` injects the harness's `profiles/<harness>/plan-mode.md` (embedded at compile time) as a `<system-reminder>` operating-context layer in every dispatch. Claude Code and Codex profiles exist today; a harness with no profile has no `--plan-mode` and an unchanged dispatch contract |
| Harness-details operator guide | (docs, not a trait method) | The README's per-harness section, e.g. [Claude Code](../README.md#claude-code-fully-wired) |

**Note on the transcript adapter (raised bar).** Baseline eval suites use `transcript_check` assertions — deterministic regex checks against a run's tool invocations (e.g. "a test command ran", "the sibling skill was loaded"). These only grade when `parse_transcript` is implemented for your harness. A harness without it still functions: those assertions grade as *unverifiable* and the `llm_judge` assertions carry the substantive measurement. But adapter richness is an explicit parity target, not optional polish — implementing or enriching `parse_transcript*` lets more of a baseline suite grade mechanically. Treat it as a goal to aim at, not a box already checked.

**Note on write enforcement (parity goal).** Eval subagents are instructed to write only inside their `outputs/` dir, but nothing in the portable contract *enforces* it — a misbehaving subagent can edit the real repo or install packages, silently tainting the run. Two layers address this: the portable `detect-stray-writes` post-pass (available to every harness, since it works off the same parsed transcript) and an opt-in harness-native `install_guard` that stages a pre-tool hook to *block* the write before it happens. Claude Code and Codex both wire this through their `PreToolUse` hook surfaces. **Harness-level tool enforcement is an explicit parity goal, not optional polish.** A harness that can express a pre-tool guard (a hook, a permission rule, a sandboxed cwd) should wire `install_guard`; until then, `detect-stray-writes` is the honest fallback.

**Note on plan-mode fidelity (residual parity goal).** `--plan-mode` injects a harness's *verbatim* plan-mode procedure as operating context, the closest a harness's eval runner can get to reproducing the wild failure where a real plan mode makes loading a skill feel redundant. It is **not** the real mode: it is still text the dispatched subagent reads, not a state the harness places it under, so a pass remains necessary-not-sufficient (the seeding ceiling is explained in the [`slow-powers`](https://github.com/slowdini/slow-powers) `evaluating-skills` skill). A harness that can dispatch an eval subagent *into* its own plan/research mode would close this gap; `--plan-mode` (a profile + renderer) is the approximation every harness can reach in the meantime.

Surface your findings inline using this template:

```
## Eval-Magic Parity Report: <harness>

### Run-mode / mechanism parity
- Mechanism(s) wired: <InSession | Cli | both> (per mechanism_for)
- End-to-end? <yes / partial — what breaks>

### Harness-adapter feature parity
- **Skill staging + native skill block** — ✅ Implemented / ⚠️ Partial / ❌ Missing / N/A
  - Where: <path or "would live at <path>">
  - Gap: <one sentence, only if Partial/Missing>

(... one block per adapter capability ...)

## Summary
- Strongest area: <capability>
- Highest-leverage gap: <capability> — <why>
- Suggested next gap to close this session: <capability>
```

Status meanings:

- **✅ Implemented** — fully wired; works the same way the reference's does, using whatever native primitive the harness provides
- **⚠️ Partial** — some scaffolding exists (e.g. the trait method is a stub or returns the default) but the capability isn't end-to-end functional
- **❌ Missing** — no implementation; users of this harness do not get this capability
- **N/A** — the capability doesn't translate. State why

The agent reports inline by default. If the user asks for a persistent artifact, write the report to `docs/parity-reports/<harness>-evals.md` (create the directory if missing).

---

## Step 5 — Pick a gap and prep to close it

Surface the report to the user and propose **one or two** gaps worth closing this session. Bias toward the smallest gap with the highest user impact — typically a `parse_transcript` impl or an operator-guide section, not a wholesale runner rework.

Once the user picks a gap:

1. Re-read the reference impl for that capability in detail (the matching `HarnessAdapter` method on `ClaudeCodeAdapter` or `CodexAdapter`, plus what it delegates to). Note the *shape* — inputs, outputs, side effects — separately from the *harness-specific mechanism* it uses.
2. **Consult your harness's own documentation, MCP servers, or built-in references** before proposing harness-specific changes. Do not guess at hook schemas, transcript formats, or native tool names. If a docs-fetch server is available, prefer it over your training data — assume your knowledge of the harness may be stale.
3. Propose an adaptation that copies the reference's shape while using your harness's native conventions — i.e. fill in your harness's `HarnessAdapter` method. State explicitly what you are copying and what you are adapting.
4. Confirm with the user before writing code.

---

## Guardrails

- **Cross-harness compatibility is enforced.** A change for your harness MUST NOT break or degrade any other harness. Keep harness-specific behavior behind your `HarnessAdapter` impl; generic run-mode code goes through the trait.
- **One problem per PR.** A parity-closing PR should wire one capability for one harness.
- **Do not fabricate features that don't exist in any harness yet.** Parity means "catch up to a reference mechanism/adapter," not "invent something new."
- **Do not guess at harness-specific details.** If your harness's docs don't confirm something, ask the user before proceeding.
- **Keep this file evergreen.** If you add a dispatch mechanism (`DispatchMechanism`) or a `HarnessAdapter` method, update Step 4a / 4b here in the same PR.
