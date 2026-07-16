# eval-magic

**One-stop CLI for running skill evals** — structured measurements of whether an agent skill actually shifts behavior.

An eval dispatches a fresh subagent twice per test case — once with the skill loaded, once without (or old version vs. new) — and grades both outputs against assertions. The pass-rate delta tells you whether the skill is worth shipping or the change is worth landing. The runner builds the workspace, stages skills for discovery, generates dispatch prompts, assembles run records from transcripts, grades, and aggregates; your agent harness supplies the one thing the runner never does itself: dispatching the subagents.

`eval-magic` ships as a dependency-less prebuilt binary under the command name `eval-magic`. Every artifact follows a documented JSON Schema, so records grade the same way regardless of where they were authored. **Claude Code and Codex CLI are fully wired harnesses today**; OpenCode has native staging support; see [Harnesses](#harnesses) for per-harness enhancement support. From inside an agent session, running an eval is as simple as: *"Install eval-magic and help me run an eval on my-skill."*

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
#    identical. The runbook carries the exact dispatch recipes for your harness.

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

The chains run in-process and stop at the first failure; re-running after a fix is safe — every sub-step skips work that's already done. The individual steps (`record-runs`, `fill-transcripts`, `detect-stray-writes`, `grade`, `aggregate`) remain callable for inspection or recovery. The per-task dispatch recipe lives in `RUNBOOK.md` and `dispatch-manifest.md`, and `ingest` reads each task's events file (`outputs/<harness>-events.jsonl`); harnesses without transcript ingest still write records by hand until their adapters land.

### Isolation grouping (which agents batch together)

`run` decides at **setup** time which evals can share an environment and which need their own, writes the plan into `dispatch.json` (a `groups[]` summary plus a per-task `group`/`eval_root`), and the runbook follows it — whoever drives the loop does no isolation reasoning themselves. By default every eval shares one group. Two things create a separate group: evals whose fixtures would clobber each other, and an eval that opts out explicitly with `"isolation": "isolated"` in `evals.json` (use it when an eval's agent *mutates* a fixture another eval reads).

Each `(group, condition)` gets its own env — `iteration-N/env-<group>-<condition>/` — so every dispatch `cd`s into a fully-isolated cwd holding only that group's fixtures, plus the staged skill for the `with_skill` arm (the control arm's env holds no skill at all). This structural split *is* the per-condition read-isolation barrier — there is no runtime switch step.

## Cost & confirmation

An eval run is not free: an N-case suite is **2N full agent sessions**, plus a judge dispatch per `llm_judge` assertion — real wall-clock time and real tokens. A subagent under test runs the real skill, and some skills write to disk, so it can write outside its sandbox.

If you are an agent driving this tool, **never kick off a run silently.** Present the user a run summary — skill, mode, eval cases, the models that will run the agents and the judge, the cost, and the guard status — and wait for explicit confirmation. Pass `--agent-model <id>` and `--judge-model <id>` to have the generated command recipes select those models when the harness adapter supports model selection (see the [Harnesses](#harnesses) table); otherwise they are recorded as provenance. Arm `--guard` unless the user actively opts out; unguarded, stray writes are only *detected* after the fact by `detect-stray-writes`, never blocked.

The judgment of *whether* a change needs an eval, and how to design cases that actually measure it, lives in the [`slow-powers`](https://github.com/slowdini/slow-powers) plugin's `evaluating-skills` skill.

## Authoring assertions

After you've seen what iteration 1 produces, add **assertions** to `evals.json` and re-grade without re-dispatching. Two types:

- **`transcript_check` — mechanical.** Regex matched against a run's tool invocations. Fast, deterministic, cheap. Use for "did the agent run X" or "did file Y get written." Requires the harness's transcript-ingest enhancement (see the [Harnesses](#harnesses) table); without it these grade as unverifiable.
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

Read `validity_warnings` **before** trusting any delta — a low skill-invocation rate, a flagged stray write, or a flagged live-source read (an arm that read the live skill source instead of its staged copy) means the result may not reflect the skill at all.

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

**Parity is only as clean as your session.** Staging controls what the runner *adds*, not what your session already *loaded*. Subagents dispatched in-process share the parent session's plugins, so an installed plugin exposing a same-named skill is still discoverable and contaminates both arms — the staging slug stops an on-disk collision, not runtime discovery. The runner can't unload a live plugin; on Claude Code it emits a build-time *plugin-shadow* warning (also surfaced in `benchmark.json`'s `validity_warnings`) that lists the per-dispatch isolation options inline. Closing it is a launch-time step for whoever dispatches.

## Harnesses

Every artifact follows a JSON Schema in [`schema/`](schema/), so a run record means the same thing regardless of which harness produced it. Harness support is **a minimal baseline plus progressive enhancements**: any harness with a headless one-shot CLI can run evals at baseline (`--no-stage` inlines the skill into each dispatch prompt, `llm_judge` grades, `detect-stray-writes` audits), and each enhancement a harness's adapter wires raises fidelity from there. What each enhancement is, why it needs harness-specific support, and what its fallback is are documented in [docs/progressive-enhancements.md](docs/progressive-enhancements.md).

**Bring your own harness:** a harness eval-magic has never seen is added with a TOML descriptor file — no code. `eval-magic harness init <name>` scaffolds the descriptor with every field explained inline (plus a notes skeleton for recording verified values), and descriptors layer (embedded built-ins → `~/.config/eval-magic/harnesses/` → `.eval-magic/harnesses/` → `--harness-file`) with field-level merge, so a project can also retune a single field of a built-in. `eval-magic harness list|show|lint` inspect and validate the registry; the authoring guide is [docs/byoh.md](docs/byoh.md). A descriptor that proves out can be upstreamed as a data-only PR and lands as a new row in the support table below — baseline first, enhancement columns flipping as each is wired.

### How dispatch works

Every eval test and judge is dispatched the same way: through the harness's one-shot CLI (`claude -p`, `codex exec`), one subprocess per task, each `cd`'d into its `(group, condition)` env and writing its events transcript to disk. `run` prepares the envs and `RUNBOOK.md`; from there an **agent session** can drive the loop (reading the runbook and shelling out each recipe) or a **human** can follow the same runbook by hand. The generated `RUNBOOK.md` / `dispatch-manifest.md` carry the exact per-task recipes for the selected harness — they, not this README, are the runtime reference for dispatch commands.

### Support

This table is the source of truth for per-harness enhancement support:

| Harness | Native staging | Dispatch recipes | Transcript ingest | Model flag | Write guard |
|---------|:--------------:|:----------------:|:-----------------:|:----------:|:-----------:|
| **Claude Code** | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Codex** | ✅ | ✅ | ✅ | ✅ | ✅ |
| **OpenCode** | ✅ | ❌¹ | ❌ | ❌ | ❌ |

¹ `run --harness opencode` stages skills and emits native dispatch prompts, but prints manual `opencode run` guidance instead of a copy-pasteable recipe.

A missing enhancement degrades fidelity, never correctness — every column has a fallback, and the `run` preflight warns naming it: without native staging, `--no-stage` inlines each `SKILL.md` into its dispatch prompt; without transcript ingest, `transcript_check` assertions grade as unverifiable and `llm_judge` carries the grading (tokens and duration go unrecorded); without a model flag, `--agent-model` / `--judge-model` are recorded as provenance only; without a write guard, `--guard` continues unguarded and `detect-stray-writes` audits after the fact.

Per-harness implementation notes for developers wiring features live in [docs/claude-notes.md](docs/claude-notes.md), [docs/codex-notes.md](docs/codex-notes.md), and [docs/opencode-notes.md](docs/opencode-notes.md).

## Documentation

| Where | What's in it |
|-------|--------------|
| `eval-magic --help` / `eval-magic <cmd> --help` | The flag-by-flag reference: every subcommand and flag, worked examples, the `--skill-dir` model, the skill-invocation meta-check |
| [docs/byoh.md](docs/byoh.md) | Bring your own harness: the `harness init` scaffold, authoring a descriptor file for an unknown harness, layering/merge rules, named capabilities, the `harness list`/`show`/`lint` workflow, and upstreaming a descriptor as a data-only PR |
| [docs/progressive-enhancements.md](docs/progressive-enhancements.md) | Development doc: the harness baseline-vs-enhancement contract — what each enhancement unlocks, why it needs harness-specific code, and its fallback |
| [docs/claude-notes.md](docs/claude-notes.md) / [docs/codex-notes.md](docs/codex-notes.md) / [docs/opencode-notes.md](docs/opencode-notes.md) | Development docs: per-harness implementation notes for working on eval-magic's harness support |
| [GitHub issues](https://github.com/slowdini/eval-magic/issues) | Planned features and known limitations |

## Bundled assets

- `schema/` — JSON Schemas for every artifact (`evals`, run records, `grading`, `stray-writes`, `benchmark`, `judge-tasks`, harness descriptors); the portable cross-harness contract, embedded in the binary
- `harnesses/` — the built-in harness descriptor files (one TOML per harness: declarative values plus named-capability references), schema-validated and embedded in the binary, plus the commented `harness init` templates
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
