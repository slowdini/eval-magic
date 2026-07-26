# Eval run — widget-skill (iteration 2, claude-code)

This runbook is for a human driving the run from a terminal. Work from this iteration directory
and copy-paste each step. The workspace is self-contained — you should not need the surrounding
repo.

- **Skill under test:** widget-skill
- **Mode:** revision — comparing `old_skill` vs `new_skill`
- **Dispatches:** 6 (the `tasks[]` array in `/work/.eval-magic/widget-skill/iteration-2/dispatch.json`)

## 1. Dispatch the eval agents, then ingest

Next: iterate the tasks[] array in dispatch.json and dispatch each task (from the env dir — `claude` has no --cd flag) with:
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR GIT_CEILING_DIRECTORIES
cd <eval-root> && claude -p --output-format stream-json --verbose --permission-mode acceptEdits --model model-x \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response your closing summary." \
  </dev/null \
  > <outputs_dir>/claude-events.jsonl \
  2> <outputs_dir>/claude-stderr.log
Then run `ingest --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness claude-code`.

`ingest` records each run, backfills transcripts, scans for stray writes, collects guarded-task
blocks into `guard-denials.json`, and grades every mechanical assertion. Inspect any denial
warning before trusting the affected task. It then prints any `llm_judge` tasks it could not
grade itself.

## 2. Dispatch the judge agents, then finalize
Dispatch each judge task from judge-tasks.json with:

```bash
JOBS=${JOBS:-4}
jq -j '.tasks[] | [.dispatch_prompt_path, .response_path, (.model // "")] | @tsv + "\u0000"' judge-tasks.json | \
  xargs -0 -P "$JOBS" -I{} sh -c '
    prompt_path="$(printf "%s" "$1" | cut -f1)"
    response_path="$(printf "%s" "$1" | cut -f2)"
    model="$(printf "%s" "$1" | cut -f3)"
    response_base="${response_path%.json}"
    mkdir -p "$(dirname "$response_path")"
    model_arg=""; [ -n "$model" ] && model_arg="--model $model"
    cd "/work/.eval-magic/widget-skill/iteration-2" && claude -p --output-format stream-json --verbose --permission-mode acceptEdits $model_arg \
      "Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers." \
      </dev/null \
      > "$response_base.claude-events.jsonl" \
      2> "$response_base.claude-stderr.log"
  ' sh {}
```

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
