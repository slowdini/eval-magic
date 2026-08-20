# Dispatch manifest — widget-skill iteration-2

Mode: revision (baseline: iteration-1)
Generated: 2026-01-01T00:00:00Z
Total dispatches: 2

## How to use this manifest

In an agent session, read `dispatch.json` (sibling of this file) instead of this manifest. Each task has a `dispatch_prompt_path` field pointing at the file that holds the full prompt — dispatch the task with a short "read this file and follow it" instruction rather than inlining the prompt — plus exact paths for `run.json` and `timing.json`.

**Requires:** harness dispatch commands are POSIX command lines, and `eval-magic dispatch` runs them itself, so the host it runs on needs a POSIX shell — on Windows, Git Bash (Git for Windows). WSL resolves a different filesystem namespace, so run eval-magic inside WSL rather than dispatching into it. Set EVAL_MAGIC_SH to select a specific `sh`.

## Dispatch

Every task is runner-driven — one-shot and scripted alike — so one command runs the whole plan from this iteration directory:

eval-magic dispatch --iteration <n> --harness <harness>

It runs `--jobs` tasks at a time, each in its own private environment, and writes each task's conversation.json. A task that already has one is skipped, so rerunning retries only what did not finish. A task exceeding `--timeout` is recorded as timed out, and a failing task is recorded while the rest of the batch continues. A conversation that stops at a scripted gate is valid eval data; a task with no conversation.json is incomplete and ingest skips it.

Harness dispatch (Cline):

`eval-magic dispatch` runs one fresh `cline --cwd <eval-root> --act --json --auto-approve true` per task. Detach stdin with `</dev/null>` so piped task data cannot become extra prompt context; capture stdout as `outputs/turn-<n>/cline-events.jsonl` and stderr as `outputs/turn-<n>/cline-stderr.log`. `eval-magic dispatch` writes `outputs/final-message.md` itself from the parsed transcript; the template's trailing jq step is a belt-and-braces copy of the terminal `run_result` event.

```bash
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR GIT_CEILING_DIRECTORIES
cline --cwd <eval-root> --act --json --auto-approve true -m model-x \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response your closing summary." \
  </dev/null \
  > <outputs_dir>/cline-events.jsonl \
  2> <outputs_dir>/cline-stderr.log; \
  jq -rj 'select(.type == "run_result") | .text' <outputs_dir>/cline-events.jsonl \
  > <outputs_dir>/final-message.md
```

Then run `eval-magic ingest --harness cline`; ingest reads each task's `outputs/turn-<n>/cline-events.jsonl`.

After all dispatches:

1. Run `eval-magic ingest --harness <harness>` — a fixed-order chain of record-runs (assembles every task's `run.json` from `dispatch.json` + the task's own `outputs/final-message.md` + the events file the harness CLI wrote under `outputs/turn-<n>/`, and backfills `timing.json` with transcript-derived tokens/duration; never clobbers an existing record), fill-transcripts, detect-stray-writes, and grade. Optional higher-fidelity timing: write `{ "total_tokens": <n>, "duration_ms": <n>, "source": "completion-event" }` from the task completion event to `timing.json` right after a dispatch — completion-event numbers always win over the backfill.
2. Run `eval-magic dispatch --judges --harness <harness>` to grade the judge tasks ingest listed, then `eval-magic finalize` for the benchmark.

On a harness without persisted transcripts, instead write each task's `run.json` (matching `skills/evaluating-skills/schema/run-record.schema.json`, enforced at runtime by grade/fill-transcripts/detect-stray-writes) and `timing.json` by hand when its subagent returns: carry over `eval_id`, `condition`, `skill_path` (`null` on the without_skill arm), `prompt`, and `files` from the task; populate `final_message` from the subagent's reply; leave `tool_invocations` as `[]`; capture `total_tokens`/`duration_ms` from the task completion event immediately — they may not be persisted anywhere else.

## Dispatches
### demo-eval / with_skill

- run.json:    /work/cond/run.json
- timing.json: /work/cond/timing.json
- conversation.json: /work/cond/conversation.json

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

The `widget-skill` skill is registered under the identifier `slow-powers-eval-2-with_skill__widget-skill` and is discoverable as a Cline skill. If you invoke it, use that identifier.
If it does not load as a Cline skill, read the skill from `/work/staged/widget-skill/SKILL.md` instead.

Available fixture files:
  - /work/fixtures/input.txt
Task environment: /work/task
Task-local scratch directory: /work/task/tmp
Framework output directory: /work/outputs

Instructions:
- Work normally on the task: you may edit existing files and create new files inside the task environment.
- Keep temporary and scratch files in the task-local scratch directory, not in a host temp directory.
- Use the framework output directory only for framework artifacts.
- After completing the task, write your final user-facing response to /work/outputs/final-message.md.
- Do not write outside the task environment.

User request:
Build me a widget.
```

### demo-eval / without_skill

- run.json:    /work/cond-b/run.json
- timing.json: /work/cond-b/timing.json
- conversation.json: /work/cond-b/conversation.json

```
You are executing a single test case for a skill evaluation framework.
Treat this as a real user request — do NOT optimize behavior for the eval.

No skill is loaded. Respond as you naturally would.

Available fixture files:
  - /work/fixtures/input.txt
Task environment: /work/task-b
Task-local scratch directory: /work/task-b/tmp
Framework output directory: /work/outputs-b

Instructions:
- Work normally on the task: you may edit existing files and create new files inside the task environment.
- Keep temporary and scratch files in the task-local scratch directory, not in a host temp directory.
- Use the framework output directory only for framework artifacts.
- After completing the task, write your final user-facing response to /work/outputs-b/final-message.md.
- Do not write outside the task environment.

User request:
Build me a widget.
```
