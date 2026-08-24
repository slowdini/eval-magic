# Eval run — {{SKILL_NAME}} (iteration {{ITERATION}}, {{HARNESS}})

This runbook is for a human driving the run from a terminal. Work from this iteration directory
and copy-paste each step. The workspace is self-contained — you should not need the surrounding
repo.

> **Requires:** {{POSIX_REQUIREMENT}}

- **Skill under test:** {{SKILL_NAME}}
- **Mode:** {{MODE}} — comparing `{{COND_A}}` vs `{{COND_B}}`
- **Dispatches:** {{NUM_TASKS}} (the `tasks[]` array in `{{DISPATCH_JSON}}`)

## 1. Dispatch the eval agents, then ingest

```
{{DISPATCH_CMD}}
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
{{INGEST_CMD}}
```

`ingest` records each run, backfills transcripts, scans for stray writes, collects guarded-task
blocks into `guard-denials.json`, and grades every mechanical assertion. Inspect any denial
warning before trusting the affected task. It then prints any `llm_judge` tasks it could not
grade itself. Each run's bounded `judge-evidence.md` combines the task, final message, diff,
conversation, tool summary, and source paths; those exact bytes are the primary input shared by
that run's judge tasks. Read `eval-magic docs judging` for its caps, truncation markers, and
retention contract.

## 2. Dispatch the judge agents, then finalize

```
{{JUDGE_CMD}}
```

Verdicts that are already present are skipped; the summary prints `N/M verdicts present` and exits
nonzero until every task has one, so rerun the same command to fill the gaps.

Then merge the verdicts and aggregate:

```
{{FINALIZE_CMD}}
```

## 3. Read the result

`finalize` writes the cross-condition benchmark to:

```
{{BENCHMARK_PATH}}
```

Read it for the per-condition pass rates and the `{{COND_A}}` − `{{COND_B}}` deltas.

## 4. Tear down

```
{{TEARDOWN_CMD}}
```
