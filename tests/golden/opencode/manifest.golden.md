# Dispatch manifest — widget-skill iteration-2

Mode: revision (baseline: iteration-1)
Generated: 2026-01-01T00:00:00Z
Total dispatches: 2

## How to use this manifest

In an agent session, read `dispatch.json` (sibling of this file) instead of this manifest. Each task has a `dispatch_prompt_path` field pointing at the file that holds the full prompt — dispatch the task with a short "read this file and follow it" instruction rather than inlining the prompt — plus exact paths for `run.json` and `timing.json`.

**Requires:** `eval-magic` supports Linux and macOS. On Windows, run `eval-magic` inside WSL; native Windows is unsupported. Git and a POSIX shell are required. Set EVAL_MAGIC_SH to select a specific `sh`.

## Dispatch

Every task is runner-driven — one-shot and scripted alike — so one command runs the whole plan from this iteration directory:

eval-magic dispatch --iteration <n> --harness <harness>

It runs `--jobs` tasks at a time, each in its own private environment, and writes each task's conversation.json. A task that already has one is skipped, so rerunning retries only what did not finish. A task exceeding `--timeout` is recorded as timed out, and a failing task is recorded while the rest of the batch continues. A conversation that stops at a scripted gate is valid eval data; a task with no conversation.json is incomplete and ingest skips it.

Harness dispatch (OpenCode):

`eval-magic dispatch` runs one fresh `opencode run --format json --auto` per task. Detach stdin with `</dev/null` so piped input is not appended to the message; capture stdout as `outputs/turn-<n>/opencode-events.jsonl` and stderr as `outputs/turn-<n>/opencode-stderr.log`.

```bash
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR GIT_CEILING_DIRECTORIES
opencode run --dir <eval-root> --format json --auto -m model-x \
  "Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response your closing summary." \
  </dev/null \
  > <outputs_dir>/opencode-events.jsonl \
  2> <outputs_dir>/opencode-stderr.log
```

Then run `eval-magic ingest --harness opencode`; OpenCode transcript ingest reads each task's `outputs/turn-<n>/opencode-events.jsonl`.

After all dispatches:

1. Run `eval-magic ingest --harness <harness>` — a fixed-order chain of record-runs (assembles every task's `run.json` from `dispatch.json`, `conversation.json`, and the harness events under `outputs/turn-<n>/`, and backfills `timing.json`; never clobbers an existing record), detect-stray-writes, and grade.
2. Run `eval-magic dispatch --judges --harness <harness>` to grade the judge tasks ingest listed, then `eval-magic finalize` for the benchmark.

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
<available_skills>
  <skill>
    <name>aux-helper</name>
    <description>Assists with auxiliary chores.</description>
  </skill>
  <skill>
    <name>widget-skill</name>
    <description>Builds widgets the house way.</description>
  </skill>
</available_skills>

<system-reminder>
PLAN STEP
</system-reminder>

You are executing a single test case for a skill evaluation framework.
Treat this as a real user request — do NOT optimize behavior for the eval.

The `widget-skill` skill is registered under the identifier `slow-powers-eval-2-with-skill-widget-skill` and is discoverable as an OpenCode skill. If you invoke it, use that identifier.
If it does not load as an OpenCode skill, read the skill from `/work/staged/widget-skill/SKILL.md` instead.

Codebase overlay files:
  - /work/overlays/input.txt
Task environment: /work/task
Task-local scratch directory: /work/task/tmp

Instructions:
- Work normally on the task: you may edit existing files and create new files inside the task environment.
- Keep temporary and scratch files in the task-local scratch directory, not in a host temp directory.
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

Codebase overlay files:
  - /work/overlays/input.txt
Task environment: /work/task-b
Task-local scratch directory: /work/task-b/tmp

Instructions:
- Work normally on the task: you may edit existing files and create new files inside the task environment.
- Keep temporary and scratch files in the task-local scratch directory, not in a host temp directory.
- Do not write outside the task environment.

User request:
Build me a widget.
```
