# Eval run — {{SKILL_NAME}} (iteration {{ITERATION}})

You are an agent in a **fresh, isolated** session. Follow this runbook top to bottom to run
the eval and produce `benchmark.json`. Everything you need is in this iteration directory —
you should not need anything from the surrounding repo.

- **Skill under test:** {{SKILL_NAME}}
- **Mode:** {{MODE}} — comparing `{{COND_A}}` vs `{{COND_B}}`
- **Dispatches:** {{NUM_TASKS}} (the `tasks[]` array in `{{DISPATCH_JSON}}`)

The two conditions run as **separate batches** in this one session: dispatch every subagent of
one batch, wait for them **all** to return, then switch conditions before dispatching the next.
Never interleave the batches — `switch-condition` removes the off-condition's staged skill, and a
subagent still in flight could observe a half-removed skill or read the wrong one.

## 1. Dispatch the `{{COND_A}}` batch

{{DISPATCH_COND_A}}

Wait for **every** one of these subagents to return before continuing.

## 2. Switch to the `{{COND_B}}` condition

This removes the `{{COND_A}}` staged skill so the `{{COND_B}}` batch cannot read it:

```
{{SWITCH_CMD}}
```

## 3. Dispatch the `{{COND_B}}` batch

{{DISPATCH_COND_B}}

Wait for **every** one of these subagents to return before continuing.

## 4. Ingest

```
{{INGEST_CMD}}
```

`ingest` records each run, backfills transcripts, scans for stray writes, and grades every
mechanical assertion. It then prints any `llm_judge` tasks it could not grade itself.

## 5. Dispatch the judge subagents, then finalize

Dispatch each judge task `ingest` listed as a subagent the same way — pass its
`agent_description` verbatim — then merge the verdicts and aggregate:

```
{{FINALIZE_CMD}}
```

## 6. Read the result

`finalize` writes the cross-condition benchmark to:

```
{{BENCHMARK_PATH}}
```

Read it for the per-condition pass rates and the `{{COND_A}}` − `{{COND_B}}` deltas. This is
the artifact the prep session resumes on.

## 7. Tear down

When you are done, remove the staged skills (and the write guard, if armed):

```
{{TEARDOWN_CMD}}
```
