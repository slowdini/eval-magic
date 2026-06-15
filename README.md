# eval-magic

**One-stop CLI for running skill evals** — structured measurements of whether an agent skill actually shifts behavior.

An eval dispatches a fresh subagent twice per test case — once with the skill loaded, once without (or old version vs. new) — and grades both outputs against assertions. The pass-rate delta tells you whether the skill is worth shipping or the change is worth landing. The runner builds the workspace, stages skills for discovery, generates dispatch prompts, assembles run records from transcripts, grades, and aggregates; your agent harness supplies the one thing the runner never does itself: dispatching the subagents.

`eval-magic` ships as a dependency-less prebuilt binary under the command name `eval-magic`. Every artifact follows a documented JSON Schema, so records grade the same way regardless of where they were authored. **Claude Code and Codex CLI are wired harnesses today**; OpenCode has foundational harness selection and staging support; see [Harnesses](#harnesses) for per-harness fidelity and caveats. From inside an agent session, running an eval is as simple as: *"Install eval-magic and help me run an eval on my-skill."*

This README is the complete operating guide: install, author cases, run both modes, drive the loop, read results, and keep a baseline. For the full flag-by-flag reference, run `eval-magic --help` (and `eval-magic <subcommand> --help`). For *when and why* to write an eval at all — the methodology, the decision to test, designing cases under pressure — see the [`slow-powers`](https://github.com/slowdini/slow-powers) plugin's `evaluating-skills` skill, which owns that craft.

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

Or download the archive for your platform from the release page directly. To build from source instead:

```bash
git clone https://github.com/slowdini/eval-magic
cd eval-magic
cargo build --release          # binary at target/release/eval-magic
./target/release/eval-magic --help
```

## How an eval works

For each test case, the runner sets up two conditions and your agent dispatches a fresh subagent into each, with clean context:

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
# 1. Build the iteration workspace (arm --guard — see Cost & confirmation).
#    Add --runs <N> to dispatch every eval N times per condition for variance
#    reduction (a per-eval "runs" field in evals.json overrides the flag).
eval-magic run --guard

# 2. Your agent dispatches each task in skills-workspace/my-skill/iteration-1/dispatch.json
#    as a fresh subagent (each reads its dispatch_prompt_path and follows it).

# 3. Assemble records, detect stray writes, grade:
eval-magic ingest \
  --subagents-dir ~/.claude/projects/<project-slug>/<session-id>/subagents/

# 4. Dispatch the judge tasks ingest lists, then finalize. If --guard is still
#    armed, finalize reminds you to run teardown-guard before editing source.
eval-magic finalize

# 5. Read skills-workspace/my-skill/iteration-1/benchmark.json, then clean up:
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

A run is one canonical workflow, the same in both modes:

```
run  →  dispatch agents  →  ingest  →  dispatch judges  →  finalize  →  teardown
```

1. **`run`** builds the iteration workspace, snapshots the `SKILL.md`, stages skills, and emits `dispatch.json` (machine-readable) alongside `dispatch-manifest.md` (human-readable).
2. **Dispatch agents.** Read `dispatch.json`. Each task object points at a `dispatch_prompt_path` (the full prompt lives in a file so you never reproduce kilobytes inline), an `agent_description` to pass through *verbatim* as the dispatch description, and the exact `run_record_path` / `timing_path`. For each task, dispatch a fresh subagent told to read the file at `dispatch_prompt_path` and follow it exactly. The `agent_description` is namespaced with the iteration and a per-run nonce (`<eval_id>:<condition>[:r<k>]:i<N>-<nonce>`; the `r<k>` segment appears only in multi-run cells, see `run --help` on `--runs`) — passing it through unchanged is what lets transcripts correlate to runs.
3. **`ingest`** (a fixed-order chain: record-runs → fill-transcripts → detect-stray-writes → grade) assembles each task's `run.json` and `timing.json` from `dispatch.json` + the subagent's `outputs/final-message.md` + the persisted transcript, scans for stray writes, and grades the `transcript_check` assertions. It stops at the judge hand-off, listing a judge task per `llm_judge` assertion.
4. **Dispatch judges.** Same pattern as step 2: dispatch a fresh subagent for each judge task to read its prompt file and write its verdict back.
5. **`finalize`** (grade `--finalize` → aggregate) merges the judge verdicts and writes `benchmark.json`. Read it. If a `--guard` marker is still live, it also reminds you to run `teardown-guard` before editing source.
6. **`teardown`** disarms the guard, removes the staged skill set, and reclaims the workspace artifacts that are safe to delete.

The chains run in-process and stop at the first failure; re-running after a fix is safe — every sub-step skips work that's already done. The individual steps (`record-runs`, `fill-transcripts`, `detect-stray-writes`, `grade`, `aggregate`) remain callable for inspection or recovery. Codex uses the same chain with `--harness codex` after each task captures `outputs/codex-events.jsonl`; un-wired harnesses still write records by hand until their adapters land.

## Cost & confirmation

An eval run is not free: an N-case suite is **2N full agent sessions**, plus a judge dispatch per `llm_judge` assertion — real wall-clock time and real tokens. A subagent under test runs the real skill, and some skills write to disk, so it can write outside its sandbox.

If you are an agent driving this tool, **never kick off a run silently.** Present the user a run summary — skill, mode, eval cases, the models that will run the agents and the judge (the runner can't observe these, so state them), the cost, and the guard status — and wait for explicit confirmation. Arm `--guard` unless the user actively opts out; unguarded, stray writes are only *detected* after the fact by `detect-stray-writes`, never blocked.

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
skills-workspace/<skill>/                # outside the skill directory, gitignore it
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

The only source file you author for evals is `<skill>/evals/evals.json` (or create it with `eval-magic init`). Keep `skills-workspace/` out of version control — it churns on every run. Snapshot retention is manual: delete `<workspace>/<skill>/snapshots/<label>/` when no longer needed.

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

The runner never dispatches the agent or judge itself, so it can't observe which models ran — pass `--agent-model` / `--judge-model` to record them as provenance (both default to `unspecified`). `NOTES.md` is optional and hand-authored; `promote-baseline` neither generates nor overwrites it.

## Environment parity

A subagent that runs an eval should start in an environment that mirrors a real install — otherwise the result depends on the operator's local install state rather than the skill being measured. Unless `--no-stage` is set, the runner produces this parity explicitly, in two parts:

1. **An available-skills block is built into every dispatch prompt**, listing the skills actually staged — normally just the skill-under-test, plus siblings only when `--skill-dir` is passed — rendered the way the harness surfaces discoverable skills to a real session, not in an eval-specific format.
2. **The skill-under-test is always staged.** It goes under a unique slug. When `--skill-dir` is passed, every *other* skill in that directory is copied at its natural name (excluding each skill's `evals/`) so cross-references resolve.

For the `without_skill` / baseline condition, the dispatch reflects "this skill is unavailable, others remain" when siblings were opted in with `--skill-dir`; otherwise it measures the skill against a clean no-skill baseline. `--bootstrap` is separate from parity: it injects product-specific framing inside the `<session-start-context>` block and does not enumerate skills.

**Parity is only as clean as your session.** Staging controls what the runner *adds*, not what your session already *loaded*. Subagents dispatched in-process share the parent session's plugins, so an installed plugin exposing a same-named skill is still discoverable and contaminates both arms — the staging slug stops an on-disk collision, not runtime discovery. The runner can't unload a live plugin; on Claude Code it emits a build-time *plugin-shadow* warning (also surfaced in `benchmark.json`'s `validity_warnings`). Closing it is a launch-time step — see [Claude Code](#claude-code-fully-wired) below.

## Harnesses

Every artifact follows a JSON Schema in [`schema/`](schema/), so a run record means the same thing regardless of which harness produced it. **Claude Code** and **Codex** are wired harnesses, with harness-specific fidelity notes below. The parity-audit framework for bringing another harness up to the supported feature set is in [docs/harness-parity.md](docs/harness-parity.md).

### Run modes

*How* an eval gets dispatched is a separate axis from *which* harness runs it. There are three run modes — pick whichever fits your account and plan:

- **Headless** — you never start an agent session. You run a series of `eval-magic` commands, and every eval test and judge is dispatched through the harness's one-shot CLI (`claude -p`, `codex exec`), writing transcripts to disk. The run ends in a written report.
- **Fully interactive** — you start an agent session and ask it to run an eval. The agent runs the `eval-magic` commands and dispatches tests and judges as **in-session subagents**, then hands you the report.
- **Hybrid** — like interactive, but the agent guides the process and issues headless CLI dispatches (`claude -p` / `codex exec`) for some or all tests and judges — useful for working through iterations.

Support today:

| Harness | Headless | Fully interactive | Hybrid |
|---------|:--------:|:-----------------:|:------:|
| **Claude Code** | ❌ not yet | ✅ | ❌ not yet |
| **Codex** | ❌ not yet | ❔ likely N/A¹ | ✅ |
| **OpenCode** | ❌ | ❌ | ❌² |

¹ Codex dispatches via subprocess (`codex exec`), not in-session subagents, so a "fully interactive" Codex mode may not translate. ² OpenCode foundational harness support is wired: `--harness opencode` stages skills under `.opencode/skills/` and emits native dispatch prompts. Transcript ingest, auto-record, and `--guard` are pending.

**Cost and billing.** Mode choice has billing consequences:

- **Claude Code, fully interactive** (Task-tool subagents) — billed under normal interactive session usage/limits (your subscription's interactive pool, or your API key).
- **Claude Code, headless / hybrid** (`claude -p`) — same token-based pricing, but on **subscription plans, starting June 15 2026**, `claude -p` (Agent SDK) usage draws from a **separate monthly Agent SDK credit pool**, distinct from interactive limits. Headless JSON output exposes `total_cost_usd` per invocation, so the runner can record per-task cost — something the in-session Task-tool path can't easily capture.
- **Codex, hybrid** (`codex exec`) — billed under your Codex usage.

**Intended end state:** all three modes first-class on Claude Code and Codex (where the mode translates), and OpenCode wired as a third harness. Progress is tracked in [GitHub issues](https://github.com/slowdini/eval-magic/issues).

### Claude Code (fully wired)

The run loop above *is* the Claude Code loop. Today this is the **fully-interactive** run mode (see [Run modes](#run-modes)) — subagents are dispatched in-session via the Task tool; the **headless** and **hybrid** (`claude -p`) modes are not yet wired. These are the Claude-Code-specific details:

**Isolating from installed plugins.** Read this first if the skill you're evaluating shares a name with one an installed, enabled plugin provides. Subagents are dispatched via the **Task tool**, so they inherit *this session's* enabled plugins — the staging slug avoids an on-disk collision but does not stop the installed copy from being discoverable, contaminating both arms (the `without_skill` arm is then not truly skill-absent). Plugins load at session start and can't be unloaded mid-session, so the runner only *detects and warns* (the plugin-shadow banner). To actually isolate, launch the session you run the eval from one of these ways — subagents inherit it:

- **Drop user-scope plugins, keep auth:** `claude --setting-sources project,local`. User-scope `enabledPlugins` isn't loaded; auth is unaffected.
- **Disable the specific plugin, then restart:** set `"enabledPlugins": { "<plugin>@<marketplace>": false }` in a settings source that loads at startup, and start a fresh session.
- **Clean config dir (strips everything):** `CLAUDE_CONFIG_DIR="$(mktemp -d)" claude`. No installed plugins or global skills load at all. Auth caveat: OAuth lives in `~/.claude.json`, which a relocated config dir may not carry — set `ANTHROPIC_API_KEY` or re-authenticate once in the fresh dir.

Project-local staged skills live in `<cwd>/.claude/skills/`, independent of installed plugins, so they still load and the meta-check still resolves the slug under all three.

**Same-session staging gotcha.** Subagents inherit *this session's* skill registry, fixed at session start. `run` stages the eval skills *mid-session*, so subagents dispatched from that **same session never discover the staged copies** — every with-skill arm silently falls back. Two valid paths: dispatch from a *fresh* session (started after staging, so the staged skills are present at session start), or run with `--no-stage` (each `SKILL.md` is inlined into its dispatch prompt, so there is no staged discovery to miss). Either way, run `detect-stray-writes` (folded into `ingest`) before trusting a staged result.

**Where transcripts live.** Claude Code persists subagent transcripts under `~/.claude/projects/<project-slug>/<parent-session-id>/subagents/`. Pass that directory as `--subagents-dir` to `ingest`. Besides out-of-bounds writes, `detect-stray-writes` also flags **live-source reads**: an arm whose subagent read the live skill source instead of its staged copy. That usually means the Skill tool couldn't resolve the staged slug yet and the agent improvised — fatal in revision mode, where the `old_skill` arm then sees new-skill content. Treat a flagged cell's arm as contaminated.

**Dispatching via the Task tool.** `dispatch.json` is a top-level object (`{ skill_name, iteration, run_nonce, …, tasks: [...] }`); iterate `tasks[]`. For each task, dispatch a fresh subagent via the Task tool with the prompt `Read the file at <dispatch_prompt_path> and follow its instructions exactly.` (substituting the task's `dispatch_prompt_path`), and pass `agent_description` *verbatim* as the description — it's namespaced `<eval_id>:<condition>:i<N>-<nonce>`, and passing it unchanged is what lets transcript correlation work. (The Task tool documents `description` as "short", but pass the full string regardless — correlation depends on the exact value.) You do **not** write `run.json`/`timing.json` yourself; the subagent writes `outputs/final-message.md`, and `ingest` (`record-runs`) assembles both records from disk. For a plan-mode-relevant skill, add `--plan-mode` to inject Claude Code's verbatim plan-mode procedure as a `<system-reminder>` operating-context layer.

### Codex

Codex's `codex exec --json` flow is the **hybrid** run mode today (see [Run modes](#run-modes)) — an agent session orchestrates while each dispatch shells out to the CLI. A fully **headless** Codex mode (eval-magic driving the whole loop with no session) is not yet wired, and a **fully-interactive** mode likely doesn't translate, since Codex dispatches via subprocess rather than in-session subagents.

Pass `--harness codex`: skills stage under repo-local `.agents/skills/` (the staged skill-under-test's frontmatter `name:` is rewritten to the eval slug so Codex's repo-local discovery sees it), and `conditions.json` / `dispatch.json` record `"harness": "codex"`. Dispatch each task with a fresh `codex exec --json` execution, capturing the event stream:

```bash
codex exec --cd <eval-root> --sandbox workspace-write --ask-for-approval never --json \
  --output-last-message <outputs_dir>/final-message.md \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to <outputs_dir>/final-message.md." \
  > <outputs_dir>/codex-events.jsonl
```

When the run was armed with `--guard`, add `--dangerously-bypass-hook-trust` to that `codex exec` command so the vetted project-local `PreToolUse` hook staged in `.codex/hooks.json` actually runs:

```bash
codex exec --cd <eval-root> --sandbox workspace-write --ask-for-approval never --dangerously-bypass-hook-trust --json \
  --output-last-message <outputs_dir>/final-message.md \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to <outputs_dir>/final-message.md." \
  > <outputs_dir>/codex-events.jsonl
```

Then ingest **without** `--subagents-dir` — the transcript source is fixed to each task's `codex-events.jsonl`:

```bash
eval-magic ingest --harness codex
```

`finalize` and `teardown` work the same with `--harness codex`. Codex results are lower fidelity than Claude Code in a few places: `transcript_check` matches parsed `item.completed` entries (`command_execution`, `file_change`, `web_search`, MCP items); the automatic `__skill_invoked` meta-check uses the LLM-judge fallback (Codex's JSONL exposes no deterministic skill-tool event); and `--plan-mode` injects Codex's plan-mode procedure as text rather than launching `codex exec` into the interactive CLI's real `/plan` mode. `--guard` stages a Codex `PreToolUse` hook that blocks out-of-sandbox `Bash` mutations and `apply_patch` targets before they run; `detect-stray-writes` remains the post-run audit. Bias Codex suites toward `llm_judge` assertions for behavior and `transcript_check` for tool events.

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
- `profiles/` — per-harness plan-mode procedure profiles (`--plan-mode`), embedded in the binary

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
