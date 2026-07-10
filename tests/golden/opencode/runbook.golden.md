# Eval run — widget-skill (iteration 2, opencode)

This runbook is for a human driving the run from a terminal. Work from this iteration directory
and copy-paste each step. The workspace is self-contained — you should not need the surrounding
repo.

- **Skill under test:** widget-skill
- **Mode:** revision — comparing `old_skill` vs `new_skill`
- **Dispatches:** 6 (the `tasks[]` array in `/work/.eval-magic/widget-skill/iteration-2/dispatch.json`)

## 1. Dispatch the eval agents, then ingest

Next: iterate the tasks[] array in dispatch.json and dispatch each task with `opencode run`. Model selection was recorded as provenance, but the OpenCode adapter has no CLI model flag wired yet. OpenCode transcript ingest is not yet wired, so assemble each task's `run.json`/`timing.json` manually (or capture `opencode run --format json` / `opencode export` output), then run `ingest --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness opencode`.

`ingest` records each run, backfills transcripts, scans for stray writes, and grades every
mechanical assertion. It then prints any `llm_judge` tasks it could not grade itself.

## 2. Dispatch the judge agents, then finalize
Dispatch each judge task `ingest` listed through the same harness CLI, capturing its transcript output, then finalize.

Then merge the verdicts and aggregate:

```
eval-magic finalize --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness opencode
```

## 3. Read the result

`finalize` writes the cross-condition benchmark to:

```
/work/.eval-magic/widget-skill/iteration-2/benchmark.json
```

Read it for the per-condition pass rates and the `old_skill` − `new_skill` deltas.

## 4. Tear down

```
eval-magic teardown --skill-dir /tmp/skills --skill widget-skill --harness opencode
```
