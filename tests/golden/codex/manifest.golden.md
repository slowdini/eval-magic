# Dispatch manifest — widget-skill iteration-2

Mode: revision (baseline: iteration-1)
Generated: 2026-01-01T00:00:00Z
Total dispatches: 2

## How to use this manifest

In an agent session, read `dispatch.json` (sibling of this file) instead of this manifest. Each task has a `dispatch_prompt_path` field pointing at the file that holds the full prompt — dispatch the task with a short "read this file and follow it" instruction rather than inlining the prompt — plus exact paths for `run.json` and `timing.json`.

After all dispatches (Codex):

Run one fresh `codex --ask-for-approval never exec --json` per task. Detach stdin with `</dev/null` so piped task data cannot become extra prompt context; capture stdout as `outputs/codex-events.jsonl` and stderr as `outputs/codex-stderr.log`.

```bash
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR GIT_CEILING_DIRECTORIES
codex --ask-for-approval never exec --cd <eval-root> --sandbox workspace-write --dangerously-bypass-hook-trust -m model-x --json \
  --output-last-message <outputs_dir>/final-message.md \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to <outputs_dir>/final-message.md." \
  </dev/null \
  > <outputs_dir>/codex-events.jsonl \
  2> <outputs_dir>/codex-stderr.log
```

Parallel dispatch from this iteration directory:

```bash
JOBS=${JOBS:-4}
jq -j '.tasks[] | .eval_root, "\u0000", .dispatch_prompt_path, "\u0000", .outputs_dir, "\u0000"' dispatch.json | \
  xargs -0 -P "$JOBS" -n 3 sh -c '
    eval_root="$1"
    prompt_path="$2"
    outputs_dir="$3"
    mkdir -p "$outputs_dir"
    unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR GIT_CEILING_DIRECTORIES
    codex --ask-for-approval never exec --cd "$eval_root" --sandbox workspace-write --dangerously-bypass-hook-trust -m model-x --json \
      --output-last-message "$outputs_dir/final-message.md" \
      "Read the file at $prompt_path and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to $outputs_dir/final-message.md." \
      </dev/null \
      > "$outputs_dir/codex-events.jsonl" \
      2> "$outputs_dir/codex-stderr.log"
  ' sh
```

Then run `eval-magic ingest --harness codex`; Codex transcript ingest reads each task's `outputs/codex-events.jsonl`.

After all dispatches:

1. Run `eval-magic ingest --harness <harness>` — a fixed-order chain of record-runs (assembles every task's `run.json` from `dispatch.json` + the task's own `outputs/final-message.md` + the events file the harness CLI wrote under `outputs/`, and backfills `timing.json` with transcript-derived tokens/duration; never clobbers an existing record), fill-transcripts, detect-stray-writes, and grade. Optional higher-fidelity timing: write `{ "total_tokens": <n>, "duration_ms": <n>, "source": "completion-event" }` from the task completion event to `timing.json` right after a dispatch — completion-event numbers always win over the backfill.
2. Dispatch the judge tasks ingest lists, then run `eval-magic finalize` for the benchmark.

On a harness without persisted transcripts, instead write each task's `run.json` (matching `skills/evaluating-skills/schema/run-record.schema.json`, enforced at runtime by grade/fill-transcripts/detect-stray-writes) and `timing.json` by hand when its subagent returns: carry over `eval_id`, `condition`, `skill_path` (`null` on the without_skill arm), `prompt`, and `files` from the task; populate `final_message` from the subagent's reply; leave `tool_invocations` as `[]`; capture `total_tokens`/`duration_ms` from the task completion event immediately — they may not be persisted anywhere else.

## Dispatches
### demo-eval / with_skill

- run.json:    /work/cond/run.json
- timing.json: /work/cond/timing.json

```
<session-start-context>
The following guidelines were loaded at session start by the slow-powers plugin
(equivalent to the SessionStart hook firing in a real user's environment):

Session guidelines: be concise.
</session-start-context>
## Skills

- aux-helper: Assists with auxiliary chores. (file: /work/staged/aux-helper/SKILL.md)
- widget-skill: Builds widgets the house way. (file: /work/staged/widget-skill/SKILL.md)

<system-reminder>
PLAN STEP
</system-reminder>

You are executing a single test case for a skill evaluation framework.
Treat this as a real user request — do NOT optimize behavior for the eval.

The `widget-skill` skill is registered under the identifier `slow-powers-eval-2-with_skill__widget-skill` and is discoverable as a Codex skill. If you invoke it, use that identifier.
If it does not load as a Codex skill, read the skill from `/work/staged/widget-skill/SKILL.md` instead.

Available fixture files:
  - /work/fixtures/input.txt
Task environment: /work/task
Framework output directory: /work/outputs

Instructions:
- Work normally on the task: you may edit existing files and create new files inside the task environment.
- Use the framework output directory only for framework artifacts.
- After completing the task, write your final user-facing response to /work/outputs/final-message.md.
- Do not write outside the task environment.

User request:
Build me a widget.
```

### demo-eval / without_skill

- run.json:    /work/cond-b/run.json
- timing.json: /work/cond-b/timing.json

```
You are executing a single test case for a skill evaluation framework.
Treat this as a real user request — do NOT optimize behavior for the eval.

No skill is loaded. Respond as you naturally would.

Available fixture files:
  - /work/fixtures/input.txt
Task environment: /work/task-b
Framework output directory: /work/outputs-b

Instructions:
- Work normally on the task: you may edit existing files and create new files inside the task environment.
- Use the framework output directory only for framework artifacts.
- After completing the task, write your final user-facing response to /work/outputs-b/final-message.md.
- Do not write outside the task environment.

User request:
Build me a widget.
```
