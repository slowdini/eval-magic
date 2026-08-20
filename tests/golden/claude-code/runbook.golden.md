# Eval run — widget-skill (iteration 2, claude-code)

This runbook is for a human driving the run from a terminal. Work from this iteration directory
and copy-paste each step. The workspace is self-contained — you should not need the surrounding
repo.

> **Requires:** harness dispatch commands are POSIX command lines, and `eval-magic dispatch` runs them itself, so the host it runs on needs a POSIX shell — on Windows, Git Bash (Git for Windows). WSL resolves a different filesystem namespace, so run eval-magic inside WSL rather than dispatching into it. Set EVAL_MAGIC_SH to select a specific `sh`.

- **Skill under test:** widget-skill
- **Mode:** revision — comparing `old_skill` vs `new_skill`
- **Dispatches:** 6 (the `tasks[]` array in `/work/.eval-magic/widget-skill/iteration-2/dispatch.json`)

## 1. Dispatch the eval agents, then ingest

```
eval-magic dispatch --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness claude-code
```

`dispatch` runs every task in its own private environment, `--jobs` of them at a time, and writes
each task's `conversation.json`. A task that already has one is skipped, so rerunning the same
command retries only what did not finish. A task that exceeds `--timeout` is recorded as timed out
rather than left to stall the campaign, and a task that fails is recorded and named while the rest
of the batch continues. A conversation that stops at a scripted gate is valid eval data, not a
failure.

```
eval-magic ingest --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness claude-code
```

`ingest` records each run, backfills transcripts, scans for stray writes, collects guarded-task
blocks into `guard-denials.json`, and grades every mechanical assertion. Inspect any denial
warning before trusting the affected task. It then prints any `llm_judge` tasks it could not
grade itself.

## 2. Dispatch the judge agents, then finalize

```
eval-magic dispatch --judges --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness claude-code
```

Verdicts that are already present are skipped; the summary prints `N/M verdicts present` and exits
nonzero until every task has one, so rerun the same command to fill the gaps.

Then merge the verdicts and aggregate:

```
eval-magic finalize --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness claude-code
```

## 3. Read the result

`finalize` writes the cross-condition benchmark to:

```
/work/.eval-magic/widget-skill/iteration-2/benchmark.json
```

Read it for the per-condition pass rates and the `old_skill` − `new_skill` deltas.

## 4. Tear down

```
eval-magic teardown --skill-dir /tmp/skills --skill widget-skill --harness claude-code
```
