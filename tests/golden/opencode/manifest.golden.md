# Dispatch manifest — widget-skill iteration-2

Mode: revision (baseline: iteration-1)
Generated: 2026-01-01T00:00:00Z
Total dispatches: 2

## How to use this manifest

In an agent session, read `dispatch.json` (sibling of this file) instead of this manifest. Each task has a `dispatch_prompt_path` field pointing at the file that holds the full prompt — dispatch the task with a short "read this file and follow it" instruction rather than inlining the prompt — plus exact paths for `run.json` and `timing.json`.

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

Available fixture files:
  - /work/fixtures/input.txt
Output directory: /work/outputs

Instructions:
- Write any files you produce into the output directory.
- After completing the task, write your final user-facing response to /work/outputs/final-message.md.
- Do not write outside the output directory.

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
Output directory: /work/outputs-b

Instructions:
- Write any files you produce into the output directory.
- After completing the task, write your final user-facing response to /work/outputs-b/final-message.md.
- Do not write outside the output directory.

User request:
Build me a widget.
```
