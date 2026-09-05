# Eval run — widget-skill (iteration 2, codex)

This runbook is for a human driving the run from a terminal. Work from this iteration directory
and copy-paste each step. The workspace is self-contained — you should not need the surrounding
repo.

> **Requires:** `eval-magic` supports Linux and macOS. On Windows, run `eval-magic` inside WSL; native Windows is unsupported. Git and a POSIX shell are required. Set EVAL_MAGIC_SH to select a specific `sh`.

- **Skill under test:** widget-skill
- **Mode:** revision — comparing `old_skill` vs `new_skill`
- **Dispatches:** 6 (the `tasks[]` array in `/work/.eval-magic/widget-skill/iteration-2/dispatch.json`)

## 1. Dispatch the eval agents, then ingest

> **Codex inside Codex:** If the same generated task command succeeds in an ordinary terminal
> with equivalent inputs and configuration, but fails inside the operator Codex session with
> `Operation not permitted`, the outer sandbox may be responsible. This error alone does not establish
> the cause; the inner sandbox cannot grant access denied by the outer process. Prefer running
> the generated `eval-magic dispatch` command from that ordinary terminal. Alternatively, approve
> or escalate the outer launch of `eval-magic dispatch` where the operator surface and policy support
> it, limited to the required workspace and process access. Keep the task's `--sandbox workspace-write`
> and eval guard enabled. See `eval-magic docs isolation` for diagnosis and limits on creating
> the inner sandbox.

```
eval-magic dispatch --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness codex
```

`dispatch` runs every task in its own private environment, `--jobs` of them at a time, and writes
each task's `conversation.json`. A task that already has one is skipped, so rerunning the same
command retries only what did not finish. A task that exceeds `--timeout` is recorded as timed out
rather than left to stall the campaign, and a task that fails is recorded and named while the rest
of the batch continues. A conversation that stops at a scripted gate is valid eval data, not a
failure. A conversation the responder stopped — because it produced no usable reply, or because it
hit `max_turns` — is recorded too, but it ended with the task unfinished; `dispatch` warns about
each one by name and cause, and `aggregate` counts them per condition in `benchmark.json`'s
`validity_warnings`. Those runs are weaker evidence than a completed one.

```
eval-magic ingest --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness codex
```

`ingest` records each run, backfills transcripts, scans for stray writes, collects guarded-task
blocks into `guard-denials.json`, and grades every mechanical assertion. Inspect any denial
warning before trusting the affected task. It then prints any `llm_judge` tasks it could not
grade itself. Each run's bounded `judge-evidence.md` combines the task, final message, diff,
conversation, tool summary, and source paths; those exact bytes are the primary input shared by
that run's judge tasks. Read `eval-magic docs judging` for its caps, truncation markers, and
retention contract.

## 2. Optional: explore paired evidence before grading

`compare` puts both conditions' evidence for one eval in a single Markdown report and prints its
path. Read that report with the driving agent to identify concrete candidate assertions. A single
comparison is exploratory evidence, not a grade or a statistically reliable result.

```
eval-magic compare --skill-dir /tmp/skills --skill widget-skill --iteration 2 --eval implement-widget
```

The commands cover every eval selected for this iteration. They require no authored assertions,
judge dispatches, or finalized benchmark.

Turn what you find into assertions in the skill's own `evals/evals.json` — the live file, not the
copy this iteration froze — then re-run the `ingest` command above to grade them. `grade` reads
assertions from that file and prints the path it read them from; everything the run was defined by
still comes from the copy. See `eval-magic docs judging`.

## 3. Dispatch the judge agents, then finalize

```
eval-magic dispatch --judges --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness codex
```

Verdicts that are already present are skipped; the summary prints `N/M verdicts present` and exits
nonzero until every task has one, so rerun the same command to fill the gaps.

Then merge the verdicts and aggregate:

```
eval-magic finalize --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness codex
```

## 4. Read the result

`finalize` writes the cross-condition benchmark to:

```
/work/.eval-magic/widget-skill/iteration-2/benchmark.json
```

Read it for the per-condition pass rates and the `old_skill` − `new_skill` deltas.

## 5. Tear down

```
eval-magic teardown --skill-dir /tmp/skills --skill widget-skill --harness codex
```
