# Eval run — {{SKILL_NAME}} (iteration {{ITERATION}}, {{HARNESS}})

This runbook is for a human driving the run from a terminal. Work from this iteration directory
and copy-paste each step. The workspace is self-contained — you should not need the surrounding
repo.

- **Skill under test:** {{SKILL_NAME}}
- **Mode:** {{MODE}} — comparing `{{COND_A}}` vs `{{COND_B}}`
- **Dispatches:** {{NUM_TASKS}} (the `tasks[]` array in `{{DISPATCH_JSON}}`)

## 1. Dispatch the eval agents, then ingest
{{DISPATCH_RECIPE}}

`ingest` records each run, backfills transcripts, scans for stray writes, and grades every
mechanical assertion. It then prints any `llm_judge` tasks it could not grade itself.

## 2. Dispatch the judge agents, then finalize
{{JUDGE_RECIPE}}

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
