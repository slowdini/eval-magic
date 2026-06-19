# Eval run — {{SKILL_NAME}} (iteration {{ITERATION}})

You are an agent in a **fresh, isolated** session. Follow this runbook top to bottom to run
the eval and produce `benchmark.json`. Everything you need is in this iteration directory —
you should not need anything from the surrounding repo.

- **Skill under test:** {{SKILL_NAME}}
- **Mode:** {{MODE}} — comparing `{{COND_A}}` vs `{{COND_B}}`
- **Dispatches:** {{NUM_TASKS}} (the `tasks[]` array in `{{DISPATCH_JSON}}`)

## 1. Dispatch the eval subagents, then ingest

{{DISPATCH_NEXT_STEPS}}

`ingest` records each run, backfills transcripts, scans for stray writes, and grades every
mechanical assertion. It then prints any `llm_judge` tasks it could not grade itself.

## 2. Dispatch the judge subagents, then finalize

Dispatch each judge task `ingest` listed as a subagent the same way — pass its
`agent_description` verbatim — then merge the verdicts and aggregate:

```
{{FINALIZE_CMD}}
```

## 3. Read the result

`finalize` writes the cross-condition benchmark to:

```
{{BENCHMARK_PATH}}
```

Read it for the per-condition pass rates and the `{{COND_A}}` − `{{COND_B}}` deltas. This is
the artifact the prep session resumes on.

## 4. Tear down

When you are done, remove the staged skills (and the write guard, if armed):

```
{{TEARDOWN_CMD}}
```
