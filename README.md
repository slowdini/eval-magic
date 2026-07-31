# eval-magic

**One-stop CLI for running skill evals** — structured measurements of whether an agent skill actually shifts behavior.

An eval dispatches a fresh subagent twice per test case — once with the skill loaded, once without (or old version vs. new) — and grades both outputs against assertions. The pass-rate delta tells you whether the skill is worth shipping or the change is worth landing. The runner builds the workspace, stages skills for discovery, generates dispatch prompts, assembles run records from transcripts, grades, and aggregates; your agent harness supplies the one thing the runner never does itself: dispatching the subagents.

`eval-magic` ships as a dependency-less prebuilt binary under the command name `eval-magic`. Every artifact follows a documented JSON Schema, so records grade the same way regardless of where they were authored. **Claude Code and Codex CLI are fully wired harnesses today**; OpenCode has native staging and transcript-ingest support; see [Harnesses](#harnesses) for per-harness enhancement support. From inside an agent session, running an eval is as simple as: *"Install eval-magic and help me run an eval on my-skill."*

This README is the complete operating guide: install, author cases, run the loop, read results, and keep a baseline. For the full flag-by-flag reference, run `eval-magic --help` (and `eval-magic <subcommand> --help`). For *when and why* to write an eval at all — the methodology, the decision to test, designing cases under pressure — see the [`slow-powers`](https://github.com/slowdini/slow-powers) plugin's `evaluating-skills` skill, which owns that craft.

## Install

`eval-magic` ships as a standalone binary named `eval-magic`; Git is its only runtime prerequisite. Each [GitHub Release](https://github.com/slowdini/eval-magic/releases) carries prebuilt binaries for macOS (Apple Silicon + Intel), Linux (x64 + ARM64), and Windows (x64), plus installer scripts.

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

For each test case, the runner sets up two conditions and a fresh native session runs each with clean context. A one-shot case uses one harness CLI call (`claude -p`, `codex exec`); a scripted case resumes that same session for each delivered follow-up. Harness details are covered under [Harnesses](#harnesses):

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

For a scenario that needs clarification before the agent can safely act, add an
ordered `turns` array. There is no turn-count cap:

```json
{
  "id": "timezone-date-fix",
  "prompt": "The displayed date is wrong. Fix it.",
  "expected_output": "Clarifies semantics before editing, then fixes the bug",
  "turns": [
    {
      "prompt": "Affected users are in US timezones.",
      "deliver_when": "agent_asks",
      "agent_response_matches": "(?i)time ?zone"
    },
    {
      "prompt": "It is a date-only field.",
      "deliver_when": "always"
    }
  ]
}
```

`always` sends the follow-up unconditionally. `agent_asks` requires the preceding
assistant round to contain `?`; an optional Rust regex is an additional compatibility
gate. If either gate fails, the conversation stops normally with
`agent_did_not_ask` or `agent_response_mismatch`, later canned turns are not sent,
and the stopped run remains gradeable. Scripted tasks run through
`eval-magic dispatch-task` as directed by the generated runbook so every round
resumes the same native harness session.

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
# 1. Build the iteration's isolated envs (the write guard arms automatically —
#    see Cost & confirmation). run stages skills into one private env per
#    (eval, condition, run) under .eval-magic/my-skill/iteration-1/, copies fixtures in, and
#    writes RUNBOOK.md. It does NOT dispatch — it prints a handoff. Add
#    --runs <N> to dispatch every eval N times per condition for variance
#    reduction (a per-eval "runs" field in evals.json overrides the flag).
eval-magic run

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

# 4. Dispatch the judge tasks ingest lists, then finalize. If the guard is still
#    armed, finalize reminds you to run teardown-guard before editing source.
eval-magic finalize

# 5. Read .eval-magic/my-skill/iteration-1/benchmark.json, then clean up:
eval-magic teardown
```

### Mode B — revision (old vs. new) — the common case

You've already edited the skill; snapshot the old version straight from git (`--ref` reads the object database without touching the working tree):

```bash
eval-magic snapshot --ref HEAD
eval-magic run --mode revision
# …then steps 2–5 as above.
```

If you snapshot *before* editing, omit `--ref` (it then reads the working tree) and run it ahead of the edit.

## The run loop

A run is one canonical workflow. `run` *prepares* the isolated envs and hands off; the runbook then drives the rest of the loop to `benchmark.json`. An agent session or a human at a terminal can drive it — the steps are identical:

```
run (prepare one private env per dispatch + RUNBOOK.md)
  └─► [runbook-driven] dispatch condition A → dispatch condition B
        → ingest → dispatch judges → finalize  ──►  benchmark.json
teardown
```

1. **`run` prepares — it does not dispatch.** It builds the iteration workspace (`iteration-N/`), snapshots the `SKILL.md`, stages skills into one private task env per `(eval, condition, run)` (`iteration-N/env-<group>-<condition>/`, with `-run-<k>` for repeated runs), copies visible fixtures in so each reads like a real repo, emits `dispatch.json` (machine-readable) alongside `dispatch-manifest.md` (human-readable), and writes `RUNBOOK.md` into `iteration-N/`. After staging and guard installation, it initializes each task as a clean Git repository, runs shadow preflight from that repository boundary, and snapshots the final-environment baseline. Then it prints a handoff, not a dispatch.
2. **Follow the runbook.** From `iteration-N/`, read `RUNBOOK.md` end to end. An agent session can drive it (*Read and follow `RUNBOOK.md`*) or you can follow it by hand — the commands are identical. It carries the exact per-task dispatch recipe plus the `ingest` / `finalize` commands, each already threaded with `--harness`.
3. **Dispatch agents (runbook-driven).** Read `dispatch.json`. Each task object points at a `dispatch_prompt_path` (the full prompt lives in a file so you never reproduce kilobytes inline), the `eval_root` env to dispatch from, and the exact `run_record_path` / `timing_path`. One-shot tasks use the harness CLI recipe. Tasks with `turns` use `eval-magic dispatch-task`, which starts the harness CLI once, resumes the same native session for each delivered follow-up, and writes a schema-validated `conversation.json`; raw events remain under `outputs/turn-N/`. The agent may edit existing source or create files anywhere inside `eval_root`; it must not write outside that task environment. Each prompt names `<eval_root>/tmp/` as the conventional task-local scratch directory instead of a host temp directory. The directory is not pre-created or reserved, and files left there remain ordinary workspace content that counts toward `diff_scope`. Conditions and repeated runs are physically isolated.
4. **`ingest`** (a fixed-order chain: record-runs → fill-transcripts → detect-stray-writes → grade) assembles each task's `run.json` and `timing.json`, scans for writes outside `eval_root`, and collects every guard block from the private envs into `guard-denials.json` even when a task produced no `run.json`. It also measures the final environment into `diff-scope.json`, grades transcript checks, and only then injects and runs held-out `command_check` assertions. It stops at the judge hand-off, listing a judge task per `llm_judge` assertion; `diff_scope` thresholds are folded in at finalize.
5. **Dispatch judges.** Same pattern as step 3: run the CLI recipe for each judge task to read its prompt file and write its verdict back.
6. **`finalize`** (grade `--finalize` → aggregate) merges judge verdicts, runner-owned command results, and `diff_scope` thresholds, then writes `benchmark.json` into `iteration-N/`, *above* the envs. Its top-level `diff_scope` object also preserves the raw per-run metrics whether or not an eval declared a scope assertion. Read it. If a guard marker is still live, it also reminds you to run `teardown-guard` before editing source.
7. **`teardown`** disarms the guard, removes the staged skill set, and reclaims the workspace artifacts that are safe to delete.

The chains run in-process and stop at the first failure; re-running after a fix is safe — every sub-step skips work that's already done. The individual steps (`record-runs`, `fill-transcripts`, `detect-stray-writes`, `grade`, `aggregate`) remain callable for inspection or recovery. The per-task dispatch recipe lives in `RUNBOOK.md` and `dispatch-manifest.md`, and `ingest` reads each task's events file (`outputs/<harness>-events.jsonl`); harnesses without transcript ingest still write records by hand until their adapters land.

### Private task environments

`run` decides the complete environment plan at **setup** time and writes it into `dispatch.json` (a `groups[]` summary plus each task's `group`/`eval_root`). Every new `(eval, condition, run)` dispatch owns a private environment so its final diff has one unambiguous baseline and parallel agents cannot contaminate one another.

Each dispatch `cd`s into a fully isolated cwd holding only its fixtures, plus the staged skill for the skill-loaded arm. Multi-run envs are suffixed `-run-<k>`. The legacy `"isolation": "shared" | "isolated"` field remains schema-valid, but both values currently receive the same task-scoped isolation.

For guarded runs, that private environment is the sole allowed write root on every host; `/tmp`, `$TMPDIR`, and other host temp locations are outside the guard boundary. eval-magic gives agents the task-local scratch guidance above but does not rewrite `TMPDIR`, `TMP`, or `TEMP`.

Git is required at run time. Each private environment has a runner-owned root `.git`, branch `work`, one clean baseline commit (`eval-magic task baseline`), and no remotes. The generated dispatch commands clear inherited Git routing variables so repository discovery stays anchored to `eval_root`; an explicit `--iteration` rebuild recreates `.git` and discards prior task branches, history, and remotes. Root `.git` fixture and held-out setup paths are reserved, case-insensitively, while nested repositories such as `vendor/.git` remain valid fixtures.

## Cost & confirmation

An eval run is not free: an N-case suite is **2N native agent sessions**, plus a judge dispatch per `llm_judge` assertion — real wall-clock time and real tokens. A scripted case can add up to one model turn per delivered follow-up in each condition (and stops early when a gate fails). `command_check` adds local command runtime but no judge tokens. A subagent under test runs the real skill, and some skills write to disk, so it can attempt to write outside its task environment.

If you are an agent driving this tool, **never kick off a run silently.** Present the user a run summary — skill, mode, eval cases, the models that will run the agents and the judge, the cost, and the guard status — and wait for explicit confirmation. Pass `--agent-model <id>` and `--judge-model <id>` to have the generated command recipes select those models when the harness adapter supports model selection (see the [Harnesses](#harnesses) table); otherwise they are recorded as provenance. The write guard arms automatically on harnesses that support it (see the same table); pass `--no-guard` only when the user actively opts out. Unguarded, stray writes are only *detected* after the fact by `detect-stray-writes`, never blocked.

The judgment of *whether* a change needs an eval, and how to design cases that actually measure it, lives in the [`slow-powers`](https://github.com/slowdini/slow-powers) plugin's `evaluating-skills` skill.

## Authoring assertions

After you've seen what iteration 1 produces, add **assertions** to `evals.json` and re-grade the compatible iteration. Four types:

- **`transcript_check` — mechanical.** `tool_invocation_matches` regex-matches tool calls; scripted runs also support `assistant_message_matches` across every assistant round. `must_precede` can be `any`, `completion_claim`, or `first_write` (the first harness-declared write/patch tool; if no write occurs, that boundary is satisfied). Fast, deterministic, cheap. Requires transcript ingest; assistant-message checks additionally require a scripted `conversation.json`.
- **`command_check` — runner-owned.** After the agent finishes, `ingest` copies optional held-out `setup_files` from `<skill>/evals/` into the same relative paths in that task's env, then runs a trusted command there through `sh -c` (Unix) or `cmd /C` (Windows). Inherited Git routing variables are cleared before optional `env` and `matrix` entries are applied, so eval authors may deliberately restore one while ordinary checks stay in the task repository. The exit code defaults to `0`; optional `expect_stdout` is a regex over complete stdout, and every expectation must pass in every cell. It works with every harness, needs no transcript or LLM judge, and still runs while the agent write guard is armed.
- **`diff_scope` — runner-owned.** Deterministically gates the agent's final-environment change with `max_files_touched`, `max_lines_changed`, or both. `max_lines_changed` is added plus removed byte-lines; `hunks` is reported but is not a threshold. Framework artifacts under the task root's `.eval-magic-outputs/` and runner-owned `.git/` are excluded, while all other newly created files count, including ignored files, caches, and nested repository metadata. Use scope as a secondary signal alongside a correctness assertion: the smallest diff is not necessarily the best change.
- **`llm_judge` — judged.** Soft criteria a model evaluates. Use for "did the response quote actual evidence." Graded by a dispatched judge subagent. Harness-independent.

```json
{
  "id": "clarified-timezone-before-edit",
  "type": "transcript_check",
  "check": "assistant_message_matches",
  "pattern": "(?i)time ?zone",
  "must_precede": "first_write"
}
```

```json
{
  "id": "all-consumers-correct",
  "type": "command_check",
  "setup_files": ["holdout/test.ts"],
  "command": "bun test ./holdout/test.ts",
  "env": {
    "CI": "1"
  },
  "matrix": {
    "TZ": ["UTC", "America/Los_Angeles", "Pacific/Kiritimati", "Europe/Berlin"]
  },
  "expect_exit_code": 0,
  "expect_stdout": "2 pass"
}
```

```json
{
  "id": "focused-change",
  "type": "diff_scope",
  "max_files_touched": 4,
  "max_lines_changed": 80
}
```

Environment names and values are strings. Names must be non-empty and contain neither `=` nor NUL; values may be empty but cannot contain NUL. `matrix` is independent of `env`: it can introduce a variable, and a matrix cell overrides a same-named fixed value. Variable names are expanded in lexical order, each value array keeps declaration order, and all cells run even after one fails. The assertion passes only when every cell passes; structured per-cell environment, exit, stdout, and stderr results are written under `command-checks/`. Values such as `TZ` are passed through without semantic validation.

`command_check.env` and `matrix` affect the runner-owned check only. Configure the earlier eval-agent dispatch separately with repeatable `run --agent-env KEY=VALUE` flags or descriptor defaults under `[dispatch.env]`; CLI entries override descriptor values by key, and the last duplicate flag wins. The resolved map applies to one-shot and every scripted round, is embedded in the generated recipes, and is recorded in clear text in `conditions.json` and `dispatch.json`, so do not put secrets there. It never applies to judge agents or `command_check`.

Neither surface has an eval-magic UTC default: an unset key inherits the operator's environment. For example, use `eval-magic run --agent-env TZ=America/Los_Angeles` for an agent-side timezone reproduction, and use a separate `command_check.env` entry when the held-out check needs the same setting. Environment variables also cannot intercept `Date.now()` or `new Date()` by themselves. Pin wall-clock-dependent tests with the test framework's fake-time support, or have the fixture consume a project-defined clock variable.

Setup paths must be relative, stay inside the task env, exist under `<skill>/evals/`, and be disjoint from the eval's visible `files`; parent/descendant overlaps are rejected so a visible directory cannot expose a held-out child. Setup paths never appear in staging, prompts, or dispatch fixture lists. Multiple command checks run in declaration order against the same task env, and matrix cells also share that env, so keep commands deterministic and non-destructive. Matrix cost multiplies with conditions and `--runs` (`conditions × runs × cells` local command executions), with no fixed cell cap. This version has no command timeout—a hung command remains operator-interruptible. Completed results under `command-checks/` are reused; `grade --overwrite` reruns the complete check or matrix.

Runner-owned assertions require an eval-magic binary that recognizes their schema. Older iterations remain readable, but adding `diff_scope` to an iteration that predates private baselines fails with rebuild guidance; an old shared-env dispatch is likewise incompatible with a newly added `command_check`.

Exact schemas are in [`schema/`](schema/); the assertion shapes and the grading output are detailed in `eval-magic grade --help`. Every with-skill run also gets an automatic **skill-invocation meta-check** — did the skill actually influence behavior? — surfaced as an `invocation_rate` per condition; a run where the skill wasn't invoked is a non-data-point, not evidence the skill is bad. Guidance on *what makes a good assertion* lives in the slow-powers `evaluating-skills` skill.

## Reading results

`finalize` writes `benchmark.json`. The headline is the **delta**:

```json
{
  "run_summary": {
    "with_skill":    { "pass_rate": { "mean": 0.83, "n": 6 }, "duration_ms": { "mean": 45000, "n": 6 }, "total_tokens": { "mean": 3800, "n": 6 } },
    "without_skill": { "pass_rate": { "mean": 0.33, "n": 6 }, "duration_ms": { "mean": 32000, "n": 6 }, "total_tokens": { "mean": 2100, "n": 6 } }
  },
  "diff_scope": {
    "with_skill": [{ "eval_id": "case-a", "files_touched": 2, "lines_added": 14, "lines_removed": 3, "hunks": 3 }],
    "without_skill": [{ "eval_id": "case-a", "files_touched": 1, "lines_added": 5, "lines_removed": 0, "hunks": 1 }]
  },
  "delta": { "pass_rate": 0.50, "duration_ms": 13000, "total_tokens": 1700 }
}
```

A skill that adds 13 seconds and 1700 tokens but improves pass rate by 50 points is probably worth it; one that doubles tokens for a 2-point gain is probably not. For Mode B the keys are `old_skill` / `new_skill`, and a positive `delta.pass_rate` means the revision is an improvement.

Token totals are harness-normalized workload metrics, not provider billing totals. For example,
Codex uses non-cached input plus output; reasoning tokens are already part of output. Duration is
available only when the harness emits a reliable duration or enough timestamps to derive one.
Always read each metric's `n`: `n: 0` means unavailable, not a measured zero, even though
`mean`/`stddev` remain numeric zero for schema compatibility. Deltas use the available samples and
`validity_warnings` flags incomplete coverage.

`diff_scope` is intentionally raw rather than averaged: each condition's array is ordered by eval id and then run index. It is captured for every new run, even when no `diff_scope` assertion is configured. Treat it as diagnostic context, not an optimization target—smaller can mean focused, but it can also mean incomplete.

Read `validity_warnings` **before** trusting any delta — mixed timing sources, incomplete timing samples, a low skill-invocation rate, a flagged stray write, a guard denial, a permission-denied tool call, missing diff-scope metrics, a flagged live-source read (an arm that read the live skill source instead of its staged copy), or a legacy task whose Git top-level resolves outside `eval_root` means the result may not reflect the skill at all. Every guard denial is reported, including a legitimate boundary block, because the rejected action changed agent behavior; inspect `guard-denials.json` for the affected task and count.

A **permission-denied tool call** is the harness refusing to run something the agent asked for — an approval-required call with nobody to approve it, an explicit deny rule, or a managed setting overriding the dispatch permission mode. The dispatch can still exit 0, so the run may silently degrade to static reasoning. `ingest` writes `permission-denials.json` (tool, normalized refusal reason, and the refused input's *keys* — values are omitted so a refused command or write cannot spill its contents) and `aggregate` raises one warning per affected task. The write guard denies through the same mechanism, so its blocks appear in that report too, attributed and left to the guard's own warning rather than counted twice. Detection is per-harness: Claude Code reads structured refusals from its event stream; Codex correlates structural tool-router rejection and `PreToolUse` block records from the paired `codex-stderr.log` capture; OpenCode recognizes its own permission-layer error strings on a refused `tool_use` event (an explicit deny rule's `PermissionDeniedError`, a headless reject's `PermissionRejectedError`, or the shared `eval guard: ` guard reason). Each parser deliberately does not classify ordinary tool failures (including ambiguous DNS and OS-process failures and tool-body errors like `oldString not found`) as permission denials. `run` preflight names the fallback for a harness that cannot detect denials.

## Workspace layout

Per skill being evaluated, the runner produces this generated workspace tree (authored `evals.json` and held-out setup sources stay under the skill's `evals/` directory):

```
.eval-magic/<skill>/                     # outside the skill directory, gitignore it
  snapshots/                             # Mode B baselines, persist across iterations
    <label>/SKILL.md
  iteration-N/
    env-g<eval>-<condition>[-run-<k>]/   # private agent cwd
      .git/                               # runner-owned clean task repository
      <fixture and task files>            # edits/new files are measured
      .eval-magic-outputs/                # framework artifacts; excluded from scope
        guard-denials.jsonl               # raw privacy-safe blocks; no patch/command bodies
        eval-<id>/<condition>/[run-<k>/]
          dispatch-prompt.txt
          final-message.md
          <harness>-events.jsonl          # one-shot raw events
          turn-N/<harness>-events.jsonl   # scripted raw round events
    eval-<id>/
      <condition-a>/                     # e.g. with_skill, old_skill
        diff-scope-baseline/             # private pre-dispatch baseline
        diff-scope.json                  # raw final-environment metrics
        command-checks/<assertion>.json  # runner-owned held-out check results
        run.json                         # portable run record
        conversation.json                # scripted completion/stop + ordered events
        timing.json                      # tokens + duration
        grading.json                     # assertion results
      <condition-b>/                     # e.g. without_skill, new_skill
        diff-scope-baseline/  diff-scope.json  command-checks/  run.json  timing.json  grading.json
    conditions.json                      # what each condition is, which SKILL.md it loaded
    stray-writes.json                    # post-hoc write/read audit
    guard-denials.json                   # guard blocks joined to dispatch task keys
    benchmark.json                       # aggregate stats
    skill-snapshot.md                    # frozen SKILL.md at run time
```

With `--runs <N>` (or a per-eval `runs` field in evals.json) above 1, each condition
nests its runs instead of holding the artifacts directly — every run is graded
independently and the benchmark's per-condition `mean`/`stddev`/`n` cover all of them:

```
      <condition-a>/
        run-1/  diff-scope-baseline/  diff-scope.json  run.json  timing.json  grading.json
        run-2/  diff-scope-baseline/  diff-scope.json  run.json  timing.json  grading.json
```

Author eval definitions in `<skill>/evals/evals.json` (or create it with `eval-magic init`); command-check setup sources also live under `<skill>/evals/`. A fixture or setup path may contain a nested `.git`, but its normalized first component cannot be `.git` because that task-root path belongs to the runner. Keep `.eval-magic/` out of version control — it churns on every run. Snapshot retention is manual: delete `<workspace>/<skill>/snapshots/<label>/` when no longer needed.

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

**Parity is only as clean as each dispatch's live environment.** Staging controls what the runner *adds*, not the user, repository, admin, or plugin skills the harness's one-shot CLI also discovers. A live copy of a logical eval skill can therefore contaminate a comparison — the staging slug stops an on-disk collision, not runtime discovery. The runner scans every `(group, condition)` environment and writes the schema-v2 `plugin-shadow.json` artifact (`schema/plugin-shadow.schema.json`). It groups sources by logical skill; distinguishes subject and sibling roles, logical names and harness runtime IDs, native and cross-harness roots, live and staged paths, affected matrix cells, and observed resolution; and carries exact per-source remediation. Subject collisions are comparison-invalid. A sibling collision is a warning only when it is symmetric across both arms of every affected group; asymmetric sibling discovery is comparison-invalid. Claude records scope precedence and namespaced plugin IDs, Codex records same-name coexistence, and OpenCode probes `opencode debug skill` only when duplicate runtime IDs require a selected-path decision. `aggregate` still reads historical unversioned artifacts.

The runner can't unload a live skill. The shared build-time *skill-shadow* banner is surfaced again in `benchmark.json`'s `validity_warnings`. When a layered harness descriptor truthfully declares `[shadow] isolates_live_sources = true`, detection still records every finding and its intrinsic severity in `plugin-shadow.json`, but `run` treats it as informational provenance and `aggregate` omits the shadow validity warnings. This is an unverified operator assertion that must cover every reported source and every initial/resumed dispatch; eval-magic does not inspect command templates or implement isolation itself.

## Harnesses

Every artifact follows a JSON Schema in [`schema/`](schema/), so a run record means the same thing regardless of which harness produced it. Harness support is **a minimal baseline plus progressive enhancements**: any harness with a headless one-shot CLI can run evals at baseline (`--no-stage` inlines the skill into each dispatch prompt, `llm_judge` grades, `detect-stray-writes` audits), and each enhancement a harness's adapter wires raises fidelity from there. What each enhancement is, why it needs harness-specific support, and what its fallback is are documented in [docs/progressive-enhancements.md](docs/progressive-enhancements.md).

**Bring your own harness:** a harness eval-magic has never seen is added with a TOML descriptor file — no code. `eval-magic harness init <name>` scaffolds the descriptor with every field explained inline (plus a notes skeleton for recording verified values), and descriptors layer (embedded built-ins → `~/.config/eval-magic/harnesses/` → `.eval-magic/harnesses/` → `--harness-file`) with field-level merge, so a project can also retune a single field of a built-in. `eval-magic harness list|show|lint` inspect and validate the registry; the authoring guide is [docs/byoh.md](docs/byoh.md). A descriptor that proves out can be upstreamed as a data-only PR and lands as a new row in the support table below — baseline first, enhancement columns flipping as each is wired.

### How dispatch works

One-shot evals and judges dispatch through the harness CLI (`claude -p`, `codex exec`), one subprocess per task. Scripted evals use the runner-owned `dispatch-task` command: it launches one subprocess per round but resumes one native session and one `eval_root` across all delivered turns. `run` prepares the envs and `RUNBOOK.md`; from there an **agent session** can drive the loop (reading the runbook and shelling out each recipe) or a **human** can follow the same runbook by hand. The generated `RUNBOOK.md` / `dispatch-manifest.md` carry the exact per-task recipes for the selected harness, including resolved `--agent-env` / `[dispatch.env]` exports — they, not this README, are the runtime reference for dispatch commands.

### Support

This table is the source of truth for per-harness enhancement support:

| Harness | Native staging | Dispatch recipes | Transcript ingest | Conversation resume | Model flag | Write guard | Shadow preflight |
|---------|:--------------:|:----------------:|:-----------------:|:-------------------:|:----------:|:-----------:|:----------------:|
| **Claude Code** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Codex** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **OpenCode** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |

Most missing enhancements degrade fidelity without blocking the run: without native staging, `--no-stage` inlines each `SKILL.md`; without transcript ingest, transcript checks grade as unverifiable and tokens/duration go unrecorded; without a model flag, models are provenance only; without a write guard or shadow preflight, the post-run audit and operator checks carry those responsibilities; without dispatch recipes, the runbook carries generic handoff guidance. Conversation resume is deliberately stricter: an eval that declares `turns` is rejected during preflight when its harness has no `[conversation]` capability, because sending canned replies into unrelated fresh sessions would produce misleading data.

**Running inside Codex itself:** invoking `eval-magic run --harness codex` from a Codex session writes `.agents/skills` (and `.codex/hooks.json` when the write guard arms). Codex's default workspace-write sandbox protects those project-local config paths, so the runner may need approval/escalation or an invocation from an external terminal. That approval is Codex's own permission boundary, not something eval-magic bypasses.

Per-harness implementation notes for developers wiring features live in [docs/claude-notes.md](docs/claude-notes.md), [docs/codex-notes.md](docs/codex-notes.md), and [docs/opencode-notes.md](docs/opencode-notes.md).

## Documentation

| Where | What's in it |
|-------|--------------|
| `eval-magic --help` / `eval-magic <cmd> --help` | The flag-by-flag reference: every subcommand and flag, worked examples, the `--skill-dir` model, the skill-invocation meta-check |
| `eval-magic docs <topic>` | The reference docs embedded in the binary — version-matched to the install and readable offline: `guide` (this README) and `byoh` (the BYOH authoring guide). Bare `eval-magic docs` lists topics |
| [docs/byoh.md](docs/byoh.md) | Bring your own harness: the `harness init` scaffold, authoring a descriptor file for an unknown harness, layering/merge rules, named capabilities, the `harness list`/`show`/`lint` workflow, and upstreaming a descriptor as a data-only PR (also printable as `eval-magic docs byoh`) |
| [docs/progressive-enhancements.md](docs/progressive-enhancements.md) | Development doc: the harness baseline-vs-enhancement contract — what each enhancement unlocks, why it needs harness-specific code, and its fallback |
| [docs/claude-notes.md](docs/claude-notes.md) / [docs/codex-notes.md](docs/codex-notes.md) / [docs/opencode-notes.md](docs/opencode-notes.md) | Development docs: per-harness implementation notes for working on eval-magic's harness support |
| [docs/README.md](docs/README.md) | Development doc: the documentation placement policy — what ships in the binary vs. stays internal |
| [GitHub issues](https://github.com/slowdini/eval-magic/issues) | Planned features and known limitations |

## Bundled assets

- `schema/` — JSON Schemas for every artifact (`evals`, run records, `grading`, `stray-writes`, `guard-denials`, `benchmark`, `judge-tasks`, harness descriptors); the portable cross-harness contract, embedded in the binary
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
