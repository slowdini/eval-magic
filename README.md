# eval-magic

**One-stop CLI for running skill evals** — structured measurements of whether an agent skill actually shifts behavior.

An eval dispatches a fresh subagent twice per test case — once with the skill loaded, once without (or old version vs. new) — and grades both outputs against assertions. The pass-rate delta tells you whether the skill is worth shipping or the change is worth landing. The runner builds the workspace, stages skills for discovery, generates dispatch prompts, assembles run records from transcripts, grades, and aggregates; your agent harness supplies the one thing the runner never does itself: dispatching the subagents.

`eval-magic` ships as a dependency-less prebuilt binary under the command name `eval-magic`. Every artifact follows a documented JSON Schema, so records grade the same way regardless of where they were authored. **Claude Code and Codex CLI are wired harnesses today**; OpenCode has foundational harness selection and staging support; see [Harnesses](#harnesses) for per-harness fidelity and caveats. From inside an agent session, running an eval is as simple as: *"Install eval-magic and help me run an eval on my-skill."*

This README is the complete operating guide: install, author cases, run the loop, read results, and keep a baseline. For the full flag-by-flag reference, run `eval-magic --help` (and `eval-magic <subcommand> --help`). For *when and why* to write an eval at all — the methodology, the decision to test, designing cases under pressure — see the [`slow-powers`](https://github.com/slowdini/slow-powers) plugin's `evaluating-skills` skill, which owns that craft.

## Install

`eval-magic` ships as a standalone binary named `eval-magic`, with no runtime dependencies. Each [GitHub Release](https://github.com/slowdini/eval-magic/releases) carries prebuilt binaries for macOS (Apple Silicon + Intel), Linux (x64 + ARM64), and Windows (x64), plus installer scripts.

macOS / Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/slowdini/eval-magic/releases/latest/download/eval-magic-installer.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/slowdini/eval-magic/releases/latest/download/eval-magic-installer.ps1 | iex"
```

Or download the archive for your platform from the release page directly. If you prefer Cargo's source-build path:

```bash
cargo install eval-magic
```

To build from a checkout instead:

```bash
git clone https://github.com/slowdini/eval-magic
cd eval-magic
cargo build --release          # binary at target/release/eval-magic
./target/release/eval-magic --help
```

## How an eval works

For each test case, the runner sets up two conditions and a fresh subagent runs each with clean context. Each subagent is dispatched through the harness's one-shot CLI (`claude -p`, `codex exec`) — the specifics per harness are covered under [Harnesses](#harnesses):

- **Mode A — new skill:** `with_skill` vs `without_skill`. Validates a brand-new skill beats baseline behavior with no skill loaded.
- **Mode B — revision (the common case):** `old_skill` vs `new_skill`. Tests a language change to an existing skill — you snapshot the old `SKILL.md`, then run both variants against the same prompts. A negative or zero `delta.pass_rate` is a signal to revert.

Each subagent's output is graded against the case's assertions, and the per-condition pass rates are aggregated into a `delta` — what the skill costs (time, tokens) and what it buys (pass-rate improvement).

## Quickstart

Your skill lives in a folder with a `SKILL.md`. Test cases live next to it in `evals/evals.json`:

```bash
cd ./skills/my-skill
eval-magic init
```

`init` prompts for a first eval id, prompt, and expected output, then writes:

```json
{
  "skill_name": "my-skill",
  "evals": [
    {
      "id": "claim-without-running",
      "prompt": "hey can you check the tests pass",
      "expected_output": "Runs the test command and quotes real output"
    }
  ]
}
```

You can also script it with `--id`, `--prompt`, and `--expected-output`. If
`evals/evals.json` already exists, `init` refuses to overwrite it unless you pass
`--force`.

By default, commands target the skill in the current directory and run it in
isolation. From elsewhere, pass `--skill ./skills/my-skill` to select one skill
without staging siblings. Pass `--skill-dir ./skills --skill my-skill` only when
you want sibling skills from that directory staged as part of the eval
environment.

### Mode A — new skill (with vs. without)

```bash
# 1. Build the iteration's isolated envs (arm --guard — see Cost & confirmation).
#    run stages skills into one env per (group, condition) under
#    .eval-magic/my-skill/iteration-1/, copies fixtures in, and writes RUNBOOK.md.
#    It does NOT dispatch — it prints a handoff. Add --runs <N> to dispatch every
#    eval N times per condition for variance reduction (a per-eval "runs" field in
#    evals.json overrides the flag).
eval-magic run --guard

# 2. Follow the runbook. From the iteration dir, read RUNBOOK.md end to end. It
#    drives the whole loop below — dispatch → ingest → dispatch judges → finalize —
#    dispatching each task through the harness CLI (`claude -p` / `codex exec`) and
#    writing benchmark.json into iteration-1/. An agent session can drive the runbook
#    ("Read and follow RUNBOOK.md"), or you can follow it by hand — the steps are
#    identical. See Claude Code below for the plugin-isolation and transcript specifics.

# Steps 3–5 are driven from the runbook — shown here for reference:

# 3. ingest assembles records, detects stray writes, and grades, stopping at the
#    judge hand-off. It reads each task's events file (outputs/<harness>-events.jsonl).
eval-magic ingest

# 4. Dispatch the judge tasks ingest lists, then finalize. If --guard is still
#    armed, finalize reminds you to run teardown-guard before editing source.
eval-magic finalize

# 5. Read .eval-magic/my-skill/iteration-1/benchmark.json, then clean up:
eval-magic teardown
```

### Mode B — revision (old vs. new) — the common case

You've already edited the skill; snapshot the old version straight from git (`--ref` reads the object database without touching the working tree):

```bash
eval-magic snapshot --ref HEAD
eval-magic run --mode revision --guard
# …then steps 2–5 as above.
```

If you snapshot *before* editing, omit `--ref` (it then reads the working tree) and run it ahead of the edit.

## The run loop

A run is one canonical workflow. `run` *prepares* the isolated envs and hands off; the runbook then drives the rest of the loop to `benchmark.json`. An agent session or a human at a terminal can drive it — the steps are identical:

```
run (prepare per-(group,condition) envs + RUNBOOK.md)
  └─► [runbook-driven] dispatch condition A → dispatch condition B
        → ingest → dispatch judges → finalize  ──►  benchmark.json
teardown
```

1. **`run` prepares — it does not dispatch.** It builds the iteration workspace (`iteration-N/`), snapshots the `SKILL.md`, stages skills into one isolated env per `(group, condition)` (`iteration-N/env-<group>-<condition>/`, the cwd each dispatch runs from), copies fixtures in so each reads like a real repo, emits `dispatch.json` (machine-readable) alongside `dispatch-manifest.md` (human-readable), and writes `RUNBOOK.md` into `iteration-N/`. Then it prints a handoff, not a dispatch.
2. **Follow the runbook.** From `iteration-N/`, read `RUNBOOK.md` end to end. An agent session can drive it (*Read and follow `RUNBOOK.md`*) or you can follow it by hand — the commands are identical. It carries the exact per-task dispatch recipe plus the `ingest` / `finalize` commands, each already threaded with `--harness`.
3. **Dispatch agents (runbook-driven).** Read `dispatch.json`. Each task object points at a `dispatch_prompt_path` (the full prompt lives in a file so you never reproduce kilobytes inline), the `eval_root` env to dispatch from, and the exact `run_record_path` / `timing_path`. For each task, run the harness CLI recipe from its `eval_root`, pointing the dispatched subagent at `dispatch_prompt_path` to read and follow exactly, and capture the events transcript into `outputs/`. Conditions are physically isolated — the `with_skill` env holds the staged skill, the control arm's env holds none — so there is no runtime "switch" step to get wrong.
4. **`ingest`** (a fixed-order chain: record-runs → fill-transcripts → detect-stray-writes → grade) assembles each task's `run.json` and `timing.json` from `dispatch.json` + the subagent's `outputs/final-message.md` + each task's events transcript, scans for stray writes, and grades the `transcript_check` assertions. It stops at the judge hand-off, listing a judge task per `llm_judge` assertion.
5. **Dispatch judges.** Same pattern as step 3: run the CLI recipe for each judge task to read its prompt file and write its verdict back.
6. **`finalize`** (grade `--finalize` → aggregate) merges the judge verdicts and writes `benchmark.json` into `iteration-N/`, *above* the envs. Read it. If a `--guard` marker is still live, it also reminds you to run `teardown-guard` before editing source.
7. **`teardown`** disarms the guard, removes the staged skill set, and reclaims the workspace artifacts that are safe to delete.

The chains run in-process and stop at the first failure; re-running after a fix is safe — every sub-step skips work that's already done. The individual steps (`record-runs`, `fill-transcripts`, `detect-stray-writes`, `grade`, `aggregate`) remain callable for inspection or recovery. The per-task dispatch recipe lives in `RUNBOOK.md` and `dispatch-manifest.md`, and `ingest` reads each task's events file (`claude-events.jsonl` / `codex-events.jsonl`); un-wired harnesses still write records by hand until their adapters land.

### Isolation grouping (which agents batch together)

`run` decides at **setup** time which evals can share an environment and which need their own, writes the plan into `dispatch.json` (a `groups[]` summary plus a per-task `group`/`eval_root`), and the runbook follows it — whoever drives the loop does no isolation reasoning themselves. By default every eval shares one group. Two things create a separate group: evals whose fixtures would clobber each other, and an eval that opts out explicitly with `"isolation": "isolated"` in `evals.json` (use it when an eval's agent *mutates* a fixture another eval reads).

Each `(group, condition)` gets its own env — `iteration-N/env-<group>-<condition>/` — so every dispatch `cd`s into a fully-isolated cwd holding only that group's fixtures, plus the staged skill for the `with_skill` arm (the control arm's env holds no skill at all). This structural split *is* the per-condition read-isolation barrier — there is no runtime switch step.

## Cost & confirmation

An eval run is not free: an N-case suite is **2N full agent sessions**, plus a judge dispatch per `llm_judge` assertion — real wall-clock time and real tokens. A subagent under test runs the real skill, and some skills write to disk, so it can write outside its sandbox.

If you are an agent driving this tool, **never kick off a run silently.** Present the user a run summary — skill, mode, eval cases, the models that will run the agents and the judge, the cost, and the guard status — and wait for explicit confirmation. Pass `--agent-model <id>` and `--judge-model <id>` to have the generated command recipes select those models when the harness adapter supports model selection (Codex today); otherwise they are recorded as provenance. Arm `--guard` unless the user actively opts out; unguarded, stray writes are only *detected* after the fact by `detect-stray-writes`, never blocked.

The judgment of *whether* a change needs an eval, and how to design cases that actually measure it, lives in the [`slow-powers`](https://github.com/slowdini/slow-powers) plugin's `evaluating-skills` skill.

## Authoring assertions

After you've seen what iteration 1 produces, add **assertions** to `evals.json` and re-grade without re-dispatching. Two types:

- **`transcript_check` — mechanical.** Regex matched against a run's tool invocations. Fast, deterministic, cheap. Use for "did the agent run X" or "did file Y get written." Requires a transcript adapter (wired for Claude Code and Codex event streams today).
- **`llm_judge` — judged.** Soft criteria a model evaluates. Use for "did the response quote actual evidence." Graded by a dispatched judge subagent. Harness-independent.

Exact schemas are in [`schema/`](schema/); the assertion shapes and the grading output are detailed in `eval-magic grade --help`. Every with-skill run also gets an automatic **skill-invocation meta-check** — did the skill actually influence behavior? — surfaced as an `invocation_rate` per condition; a run where the skill wasn't invoked is a non-data-point, not evidence the skill is bad. Guidance on *what makes a good assertion* lives in the slow-powers `evaluating-skills` skill.

## Reading results

`finalize` writes `benchmark.json`. The headline is the **delta**:

```json
{
  "run_summary": {
    "with_skill":    { "pass_rate": { "mean": 0.83 }, "duration_ms": { "mean": 45000 }, "total_tokens": { "mean": 3800 } },
    "without_skill": { "pass_rate": { "mean": 0.33 }, "duration_ms": { "mean": 32000 }, "total_tokens": { "mean": 2100 } },
    "delta": { "pass_rate": 0.50, "duration_ms": 13000, "total_tokens": 1700 }
  }
}
```

A skill that adds 13 seconds and 1700 tokens but improves pass rate by 50 points is probably worth it; one that doubles tokens for a 2-point gain is probably not. For Mode B the keys are `old_skill` / `new_skill`, and a positive `delta.pass_rate` means the revision is an improvement.

Read `validity_warnings` **before** trusting any delta — a low skill-invocation rate (or a flagged stray write) means the result may not reflect the skill at all.

## Workspace layout

Per skill being evaluated, the runner produces this tree (everything but `evals/evals.json` is generated):

```
.eval-magic/<skill>/                     # outside the skill directory, gitignore it
  snapshots/                             # Mode B baselines, persist across iterations
    <label>/SKILL.md
  iteration-N/
    eval-<id>/
      <condition-a>/                     # e.g. with_skill, old_skill
        outputs/                         # files the subagent produced
        run.json                         # portable run record
        timing.json                      # tokens + duration
        grading.json                     # assertion results
      <condition-b>/                     # e.g. without_skill, new_skill
        outputs/  run.json  timing.json  grading.json
    conditions.json                      # what each condition is, which SKILL.md it loaded
    benchmark.json                       # aggregate stats
    skill-snapshot.md                    # frozen SKILL.md at run time
```

With `--runs <N>` (or a per-eval `runs` field in evals.json) above 1, each condition
nests its runs instead of holding the artifacts directly — every run is graded
independently and the benchmark's per-condition `mean`/`stddev`/`n` cover all of them:

```
      <condition-a>/
        run-1/  outputs/  run.json  timing.json  grading.json
        run-2/  outputs/  run.json  timing.json  grading.json
```

The only source file you author for evals is `<skill>/evals/evals.json` (or create it with `eval-magic init`). Keep `.eval-magic/` out of version control — it churns on every run. Snapshot retention is manual: delete `<workspace>/<skill>/snapshots/<label>/` when no longer needed.

## Version-controlled baselines

The workspace tree is ephemeral, but two parts of a *canonical* run are worth committing: the `benchmark.json` delta (the "this skill earns its place" number) and the per-run `grading.json` rationales (why each assertion passed or failed). Promote them into the skill's tracked `evals/baseline/`:

```bash
eval-magic promote-baseline \
  [--label <tag>] [--agent-model <id>] [--judge-model <id>]
```

```
<skill>/evals/baseline/
  BASELINE.md                          # provenance: mode, iteration, models, timestamp
  benchmark.json                       # the committed delta
  grading/<eval-id>__<condition>.json  # judge rationales per run
  NOTES.md                             # optional, hand-authored — forward-looking observations
```

Pass `run --agent-model` / `run --judge-model` while the model choice is fresh: CLI-dispatch harnesses use those values in generated command recipes when supported, and every harness records them in `conditions.json` for `promote-baseline` (both default to `unspecified`). `promote-baseline --agent-model` / `--judge-model` still override the recorded values when promoting a benchmark. `NOTES.md` is optional and hand-authored; `promote-baseline` neither generates nor overwrites it.

## Environment parity

A subagent that runs an eval should start in an environment that mirrors a real install — otherwise the result depends on the operator's local install state rather than the skill being measured. Unless `--no-stage` is set, the runner produces this parity explicitly, in two parts:

1. **An available-skills block is built into every dispatch prompt**, listing the skills actually staged — normally just the skill-under-test, plus siblings only when `--skill-dir` is passed — rendered the way the harness surfaces discoverable skills to a real session, not in an eval-specific format.
2. **The skill-under-test is always staged.** It goes under a unique slug. When `--skill-dir` is passed, every *other* skill in that directory is copied at its natural name (excluding each skill's `evals/`) so cross-references resolve.

For the `without_skill` / baseline condition, the dispatch reflects "this skill is unavailable, others remain" when siblings were opted in with `--skill-dir`; otherwise it measures the skill against a clean no-skill baseline. `--bootstrap` is separate from parity: it injects product-specific framing inside the `<session-start-context>` block and does not enumerate skills.

**Parity is only as clean as your session.** Staging controls what the runner *adds*, not what your session already *loaded*. Subagents dispatched in-process share the parent session's plugins, so an installed plugin exposing a same-named skill is still discoverable and contaminates both arms — the staging slug stops an on-disk collision, not runtime discovery. The runner can't unload a live plugin; on Claude Code it emits a build-time *plugin-shadow* warning (also surfaced in `benchmark.json`'s `validity_warnings`). Closing it is a launch-time step — see [Claude Code](#claude-code-fully-wired) below.

## Harnesses

Every artifact follows a JSON Schema in [`schema/`](schema/), so a run record means the same thing regardless of which harness produced it. **Claude Code** and **Codex** are wired harnesses, with harness-specific fidelity notes below. The parity-audit framework for bringing another harness up to the supported feature set is in [docs/harness-parity.md](docs/harness-parity.md).

### How dispatch works

Every eval test and judge is dispatched the same way: through the harness's one-shot CLI (`claude -p`, `codex exec`), one subprocess per task, each `cd`'d into its `(group, condition)` env and writing its events transcript to disk. `run` prepares the envs and `RUNBOOK.md`; from there an **agent session** can drive the loop (reading the runbook and shelling out each recipe) or a **human** can follow the same runbook by hand.

Support today:

| Harness | CLI dispatch | Transcript ingest | `--guard` |
|---------|:------------:|:-----------------:|:---------:|
| **Claude Code** | ✅ | ✅ | ✅ |
| **Codex** | ✅ | ✅ | ✅ |
| **OpenCode** | ⚠️ partial¹ | ❌ | ❌ |

¹ OpenCode has foundational harness support: `--harness opencode` stages skills under `.opencode/skills/` and emits native dispatch prompts, but eval-magic does not yet drive OpenCode dispatches or ingest their transcripts.

### Claude Code

The run loop above *is* the Claude Code loop: each eval test and judge is dispatched through the `claude -p` one-shot CLI. `eval-magic run` only *prepares* the isolated envs (`.eval-magic/<skill>/iteration-N/env-<group>-<condition>/`) and writes `RUNBOOK.md` into `iteration-N/`, then prints a handoff. An agent session can drive the runbook (*Read and follow RUNBOOK.md*), or you can follow it by hand — either way each task shells out the same `claude -p` recipe, and `ingest` → `finalize` assemble `benchmark.json`. These are the Claude-Code-specific details:

**Isolating from installed plugins.** Read this first if the skill you're evaluating shares a name with one an installed, enabled plugin provides. Each `claude -p` dispatch loads *your* user/global plugins and the global skills dir from its Claude config — the staging slug avoids an on-disk collision but does not stop the installed copy from being discoverable, contaminating both arms (the `without_skill` arm is then not truly skill-absent). The runner can only *detect and warn* (the plugin-shadow banner). To actually isolate, constrain the config each `claude -p` dispatch loads, one of these ways:

- **Drop user-scope plugins, keep auth:** add `--setting-sources project,local` to the dispatch. User-scope `enabledPlugins` isn't loaded; auth is unaffected.
- **Disable the specific plugin:** set `"enabledPlugins": { "<plugin>@<marketplace>": false }` in a settings source the dispatch loads.
- **Clean config dir (strips everything):** run each dispatch under `CLAUDE_CONFIG_DIR="$(mktemp -d)"`. No installed plugins or global skills load at all. Auth caveat: OAuth lives in `~/.claude.json`, which a relocated config dir may not carry — set `ANTHROPIC_API_KEY` or re-authenticate once in the fresh dir.

Project-local staged skills live in each env at `.claude/skills/`, independent of installed plugins, so they still load and the meta-check still resolves the slug under all three.

**Discovery is structural.** Claude Code discovers the project-local skills present in the dispatch's cwd. Because `eval-magic run` builds each env's `.claude/skills/` *before* the dispatch runs, the staged skills are present from the start — there is no mid-session staging hazard. `--no-stage` remains for harnesses without project-local skill discovery: each `SKILL.md` is inlined into its dispatch prompt instead of staged. Regardless, run `detect-stray-writes` (folded into `ingest`) before trusting a result.

**Dispatching via `claude -p`.** `dispatch.json` is a top-level object (`{ skill_name, iteration, run_nonce, …, tasks: [...] }`); iterate `tasks[]`. `run` prints (and `dispatch-manifest.md` / `RUNBOOK.md` carry) a `claude -p` recipe per task:

```bash
cd <eval-root> && claude -p --output-format stream-json --verbose --permission-mode acceptEdits \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly. …" \
  </dev/null \
  > <outputs_dir>/claude-events.jsonl \
  2> <outputs_dir>/claude-stderr.log
```

Three details differ from Codex's `codex exec`: `--output-format stream-json` **requires `--verbose`** in `-p` mode; `claude` has **no `--cd` flag**, so each dispatch must run from its env dir (`cd <eval-root> &&`) — staged-skill discovery is cwd-relative, so getting this wrong makes the `with_skill` arm behave like `without_skill`; and there is **no `--output-last-message`**, so the final message is recovered from the stream-json `result` event rather than a file. Detach stdin with `</dev/null` so a permission prompt can't block on a TTY. You do **not** write `run.json`/`timing.json` yourself; `eval-magic ingest --harness claude-code` reads each task's `outputs/claude-events.jsonl` (the `-p` stream-json transcript) to populate `tool_invocations`, tokens, duration, and the final message. For a plan-mode-relevant skill, add `--plan-mode` to inject the shared plan-mode procedure as a `<system-reminder>` operating-context layer.

Besides out-of-bounds writes, `detect-stray-writes` also flags **live-source reads**: an arm whose subagent read the live skill source instead of its staged copy. That usually means the Skill tool couldn't resolve the staged slug and the agent improvised — fatal in revision mode, where the `old_skill` arm then sees new-skill content. Treat a flagged cell's arm as contaminated.

**Write guard.** `--guard` stages a `PreToolUse` hook in each env's `.claude/settings.local.json`; because every `claude -p` dispatch runs from its env (`cd <eval-root>`), it loads and enforces the hook (the recipe never passes `--bare`, which would skip hook discovery). A deny aborts the offending dispatch; `detect-stray-writes` (folded into `ingest`) remains the after-the-fact backstop.

### Codex

Codex dispatches each task through the `codex --ask-for-approval never exec --json` one-shot CLI — the same single dispatch path as Claude Code (see [How dispatch works](#how-dispatch-works)). An agent session can drive the runbook, or you can follow it by hand; either way each test and judge shells out the same `codex exec` recipe.

Pass `--harness codex`: skills stage under repo-local `.agents/skills/` (the staged skill-under-test's frontmatter `name:` is rewritten to the eval slug so Codex's repo-local discovery sees it), and `conditions.json` / `dispatch.json` record `"harness": "codex"`. Dispatch each task with a fresh `codex --ask-for-approval never exec --json` execution, capturing the event stream:

```bash
codex --ask-for-approval never exec --cd <eval-root> --sandbox workspace-write --json \
  --output-last-message <outputs_dir>/final-message.md \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to <outputs_dir>/final-message.md." \
  </dev/null \
  > <outputs_dir>/codex-events.jsonl \
  2> <outputs_dir>/codex-stderr.log
```

When `run --agent-model <id>` is set, the generated Codex recipes insert `-m <id>` before `--json`:

```bash
codex --ask-for-approval never exec --cd <eval-root> --sandbox workspace-write -m <agent-model> --json \
  --output-last-message <outputs_dir>/final-message.md \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to <outputs_dir>/final-message.md." \
  </dev/null \
  > <outputs_dir>/codex-events.jsonl \
  2> <outputs_dir>/codex-stderr.log
```

When the run was armed with `--guard`, add `--dangerously-bypass-hook-trust` to that `codex --ask-for-approval never exec` command so the vetted project-local `PreToolUse` hook staged in `.codex/hooks.json` actually runs:

```bash
codex --ask-for-approval never exec --cd <eval-root> --sandbox workspace-write --dangerously-bypass-hook-trust --json \
  --output-last-message <outputs_dir>/final-message.md \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to <outputs_dir>/final-message.md." \
  </dev/null \
  > <outputs_dir>/codex-events.jsonl \
  2> <outputs_dir>/codex-stderr.log
```

The `</dev/null` redirect matters when dispatching in parallel from a pipe (for example with `xargs -P`): without it, Codex treats piped stdin as additional prompt context. `codex-stderr.log` keeps progress/status text such as stdin notices out of the JSONL transcript.

Then ingest **without** `--subagents-dir` — the transcript source is fixed to each task's `codex-events.jsonl`:

```bash
eval-magic ingest --harness codex
```

Judge tasks follow the same model-selection rule. `run --judge-model <id>` becomes the default `model` in `judge-tasks.json`; an individual `llm_judge.model` overrides it. The Codex judge recipe reads each task's resolved `model` and passes `-m "$model"` only when one is present.

`finalize` and `teardown` work the same with `--harness codex`. Codex results are lower fidelity than Claude Code in a few places: `transcript_check` matches parsed `item.completed` entries (`command_execution`, `file_change`, `web_search`, MCP items); the automatic `__skill_invoked` meta-check uses the LLM-judge fallback (Codex's JSONL exposes no deterministic skill-tool event); and `--plan-mode` injects the shared plan-mode procedure as text rather than launching `codex exec` into the interactive CLI's real `/plan` mode. `--guard` stages a Codex `PreToolUse` hook that blocks out-of-sandbox `Bash` mutations and `apply_patch` targets before they run; `detect-stray-writes` remains the post-run audit. Bias Codex suites toward `llm_judge` assertions for behavior and `transcript_check` for tool events.

When running `eval-magic run --harness codex` from inside Codex itself, staging writes `.agents/skills`; adding `--guard` also writes `.codex/hooks.json`. Those project-local Codex config paths are protected by Codex's default workspace-write sandbox, so the runner may need approval/escalation or an external terminal invocation. That approval is Codex's own permission boundary, not something eval-magic bypasses.

### OpenCode

OpenCode is wired for **foundational harness selection and staging**. Pass `--harness opencode`: skills stage under repo-local `.opencode/skills/` (OpenCode's native project-local skills directory), the staged skill-under-test's frontmatter `name:` is rewritten to a sanitized OpenCode-valid slug, and `conditions.json` / `dispatch.json` record `"harness": "opencode"`. The dispatch prompt renders OpenCode's native `<available_skills>` XML block and a plan-mode approximation via `<system-reminder>`.

OpenCode skill names must be lowercase alphanumeric with single-hyphen separators, match the containing directory name, and be 1–64 characters. The runner sanitizes the eval-generated slug for the staged copy; sibling skills are staged at their natural names and must already satisfy OpenCode's naming rules.

**Dispatching.** Eval-magic does not yet drive OpenCode dispatches automatically. Iterate `tasks[]` in `dispatch.json` and dispatch each task with `opencode run`, passing the prompt at `dispatch_prompt_path`. Capture the result and assemble `run.json`/`timing.json` manually, or record `opencode run --format json` / `opencode export` output for the upcoming transcript adapter.

**Fidelity notes.** Transcript ingest, auto-record, and `--guard` are not yet wired for OpenCode. The automatic `__skill_invoked` meta-check uses the LLM-judge fallback until a transcript adapter lands, and `transcript_check` assertions grade as *unverifiable*. `detect-stray-writes` still audits out-of-bounds writes from any parsed transcript. `--guard` is rejected with a clear message for OpenCode; use `detect-stray-writes` as the audit fallback.

## Documentation

| Where | What's in it |
|-------|--------------|
| `eval-magic --help` / `eval-magic <cmd> --help` | The flag-by-flag reference: every subcommand and flag, worked examples, the `--skill-dir` model, the skill-invocation meta-check |
| [docs/harness-parity.md](docs/harness-parity.md) | The parity-audit framework for bringing a new harness up to the supported feature set |
| [GitHub issues](https://github.com/slowdini/eval-magic/issues) | Planned features and known limitations |

## Bundled assets

- `schema/` — JSON Schemas for every artifact (`evals`, run records, `grading`, `stray-writes`, `benchmark`, `judge-tasks`); the portable cross-harness contract, embedded in the binary
- `profiles/` — the shared plan-mode procedure profile (`--plan-mode`) and runbook templates, embedded in the binary

## Development

```sh
cargo build              # debug build
cargo run -- --help      # explore the command surface
cargo test               # run tests (also installs git hooks via cargo-husky)
cargo fmt --all          # format
cargo clippy --all-targets --all-features -- -D warnings   # lint
```

Git hooks are installed automatically on first `cargo test`: pre-commit runs `fmt --check` + `clippy`, pre-push runs the test suite.

## License

MIT
