# Eval run — widget-skill (iteration 2, claude-code)

This runbook is for a human driving the run from a terminal. Work from this iteration directory
and copy-paste each step. The workspace is self-contained — you should not need the surrounding
repo.

> **Requires:** eval-magic's dispatch and judge recipes are POSIX command lines built on `jq`, `xargs`, `tr`, and `wc`. Run them in a POSIX shell with `jq` installed — on Windows, use Git Bash (Git for Windows) or WSL. Set EVAL_MAGIC_SH to select a specific `sh`.

- **Skill under test:** widget-skill
- **Mode:** revision — comparing `old_skill` vs `new_skill`
- **Dispatches:** 6 (the `tasks[]` array in `/work/.eval-magic/widget-skill/iteration-2/dispatch.json`)

## 1. Dispatch the eval agents, then ingest

Next: iterate the tasks[] array in dispatch.json and dispatch each task (from the env dir — `claude` has no --cd flag) with:
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR GIT_CEILING_DIRECTORIES
cd <eval-root> && claude -p --output-format stream-json --verbose --permission-mode bypassPermissions --model model-x \
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
Existing nonempty response files are skipped; delete one to dispatch that judge again.
The final `N/M verdicts present` summary exits nonzero until every task has one.

```bash
JOBS=${JOBS:-4}
jq -r '.tasks[] | .dispatch_prompt_path, .response_path, ("model=" + (.model // ""))' judge-tasks.json \
  | tr -d '\r' \
  | tr '\n' '\0' \
  | xargs -0 -P "$JOBS" -n 3 sh -c '
    prompt_path="$1"
    response_path="$2"
    model="${3#model=}"
    if [ -s "$response_path" ]; then exit 0; fi
    response_base="${response_path%.json}"
    mkdir -p "$(dirname "$response_path")"
    model_arg=""; [ -n "$model" ] && model_arg="--model $model"
    cd "/work/.eval-magic/widget-skill/iteration-2" && claude -p --output-format stream-json --verbose --permission-mode bypassPermissions $model_arg \
      "Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers." \
      </dev/null \
      > "$response_base.claude-events.jsonl" \
      2> "$response_base.claude-stderr.log"
  ' sh
judge_dispatch_status=$?
judge_total=$(jq '.tasks | length' judge-tasks.json | tr -d '\r')
judge_present=$(
  jq -r '.tasks[].response_path' judge-tasks.json \
    | tr -d '\r' \
    | while IFS= read -r response_path; do
        if [ -s "$response_path" ]; then printf '%s\n' "$response_path"; fi
      done \
    | wc -l \
    | tr -d '[:space:]'
)
printf '%s/%s verdicts present\n' "$judge_present" "$judge_total"
[ "$judge_dispatch_status" -eq 0 ] && [ "$judge_present" -eq "$judge_total" ]
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
