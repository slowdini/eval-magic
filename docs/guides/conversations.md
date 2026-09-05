# Multi-turn conversations

Most evals are one shot: the agent gets a prompt, works, and answers. Some tasks
are not like that. A realistic request often needs a decision from the user part
way through, and an eval that cannot supply one measures an agent talking to a
wall.

An eval declares one of two ways to supply those answers. They are alternatives,
not layers — declaring both is a configuration error.

- **`turns`** — an authored script. You say exactly what the user says, and in
  what order. Use it when the exchange is the thing under test and you want it
  identical in every run.
- **`responder`** — a policy that derives each answer from what the agent just
  said. Use it when you do not know what the agent will ask, which is the normal
  case for a real task against a real codebase.

Declaring neither leaves the eval one shot.

Both need a harness that can resume its own session, so a follow-up reaches the
agent that asked rather than a fresh one. `eval-magic run` rejects the eval up
front when the selected harness cannot. `eval-magic harness list` names the
`conversation-resume` capability for every harness that has it. An eval that
begins in the harness's plan mode needs the `plan-mode` capability as well; see
"Starting in plan mode" below.

## The responder

```json
{
  "id": "add-request-caching",
  "prompt": "Requests to the pricing API are slow. Can you add caching?",
  "expected_output": "A working cache with the pricing endpoint under 100ms.",
  "responder": { "type": "llm", "max_turns": 8 }
}
```

- **`type`** is required. `llm` is the only responder: a small model, consulted
  once after every round through the same harness as the agent under test.
  Choose it with `eval-magic run --responder-model`; omit that flag and the
  consultation runs on the harness's default model.
- **`max_turns`** bounds how many follow-ups the responder may synthesize. The
  opening prompt is not one of them. It defaults to 8.

Every turn the responder produces is recorded in the run's `conversation.json`
with an `origin` naming the responder and, when it offered one, its one-line
reason for answering that way. A turn the eval authored — the opening prompt, a
scripted turn — carries no `origin`; the one turn the runner itself authors, a
plan-mode eval's approval, carries `origin.runner` instead.

The full prompt and verdict of every consultation are kept on disk under the
run's `responder/turn-<n>/`, so you can audit what the responder was shown and
what it wrote without rerunning anything.

## What the responder is shown

**Only what the agent already knows.** Each consultation carries the eval's
opening `prompt`, every reply the responder has already given, and the agent's
last message. It does not carry `expected_output` and it does not carry the
assertions: those are the grading criteria, and a responder that had read them
could hand the agent the rubric.

It is told to answer as the person who asked for the work — take whatever the
agent marked as recommended, else the simplest option and the least work; add no
requirements; invent no facts; write no code; keep it short.

It answers with one of three verdicts:

| Verdict | What it means |
| --- | --- |
| `answer` | What the user says next. Delivered as the following turn. |
| `done` | The agent is reporting the task finished and waiting on nothing. |
| `cannot_answer` | It could not answer without inventing something. |

Because the responder decides `done`, completion is a judgement rather than the
absence of a question mark — and the judgement is recorded with its reason, so
a run that stopped early is legible rather than mysterious.

## What is never delivered

A reply that fails any of these checks is not sent to the agent. The run stops
instead, because an undelivered reply is a loud, greppable stop, while a bad one
enters the transcript as an ordinary user turn and is graded as though the
exchange really happened.

| Rejected when the reply | Recorded cause |
| --- | --- |
| is blank | `empty_reply` |
| runs past 2000 bytes | `reply_too_long` |
| contains a fenced code block | `reply_contains_code` |
| repeats the previous reply verbatim | `reply_repeated` |

The length and code rules are the same rule twice: a simulated user answers in
sentences, so anything longer means the responder started doing the agent's work,
and crediting the agent under test with work it did not do would corrupt the
result. A repeat means the exchange is circling, and spending the remaining
turns on it would only reach the same place more expensively.

A consultation that never produces a reply stops the run the same way, with its
own cause: `declined` when the responder honestly refused, and
`dispatch_failed`, `dispatch_timed_out`, `missing_verdict`, or
`malformed_verdict` when something broke. One outcome, because the run ended
mid-task either way; separate causes, because an honest refusal and a broken
dispatch call for different fixes.

## How a conversation ends

| Recorded as | When |
| --- | --- |
| `completed` | The responder judged the agent finished. The run stops rather than burning its remaining turns. |
| `stopped`, `responder_cannot_answer` | The responder produced no usable reply. `responder_outcome.cause` says why. |
| `stopped`, `max_turns_reached` | The agent was still asking at the bound. |
| `stopped`, `plan_not_presented` | A plan-mode session ended its planning phase with no plan to approve. |
| `timed_out` | The task outran `dispatch --timeout`. |

A `stopped` conversation is recorded, not failed: `dispatch` exits zero and
`ingest` still records the run. But both responder stops end the conversation
with the task unfinished, so `dispatch` warns about each one by name and
`aggregate` counts them per condition in `benchmark.json`'s
`validity_warnings`. That count is the one to read first: one arm truncated more
often than the other is a threat to the comparison, not just to the run.

## Dispatch timing

Every task driven by `eval-magic dispatch` records `duration_ms` in its completed, stopped, or
timed-out `conversation.json` file. This is a runner-owned monotonic measurement of time spent
inside the eval-agent harness subprocess. For a multi-turn task, it is the sum of the initial
dispatch and every resumed round.

The measurement excludes responder consultations, judge dispatches, time waiting in the task
queue, and runner bookkeeping. A timeout retains the elapsed time spent waiting for the killed
harness process, while a harness command that fails writes no completion artifact.

During `ingest`, `timing.json` becomes the canonical location for the measurement. New timing files
record provenance per metric: transcript-normalized tokens use `token_source: "transcript"`, and
runner duration uses `duration_source: "runner"`. `run.json` does not duplicate the runner timing.
Historical or externally produced completion artifacts without `duration_ms` fall back to a native
transcript duration when the harness exposes one; existing `timing.json` files are preserved unless
you pass `--overwrite`.

## Cross-harness behaviour

The responder needs no per-harness support and no descriptor field. It reads
`TranscriptSummary::final_text`, which every harness's parser already
normalizes, replies through the existing `{prompt_arg}` slot, and runs its own
consultations through the same `[dispatch].exec_template` a judge uses. Any
harness that can resume a session can run a responder eval.

Nothing is read from a harness-native question tool, and nothing needs to be: a
dispatch runs headless with stdin detached, so a tool that asks the user has no
channel to be answered on. Free text is the only mechanism that fits, and it
happens to be the portable one.

Consultations run in the run's own `responder/` directory, which sits above the
task environment. That is deliberate — a consultation must not be able to write
into the codebase under measurement, and must not pick up that codebase's
`CLAUDE.md` or `AGENTS.md` as instructions to itself.

## Scripted turns

```json
{
  "id": "clarify-before-editing",
  "prompt": "The due date is wrong. Fix it.",
  "expected_output": "Asks which timezone before editing.",
  "turns": [
    {
      "prompt": "The affected users are all in US timezones.",
      "deliver_when": "agent_asks",
      "agent_response_matches": "(?i)time ?zone"
    },
    { "prompt": "It is a date-only field.", "deliver_when": "always" }
  ]
}
```

Each turn is delivered in order. `deliver_when: always` delivers
unconditionally; `agent_asks` delivers only when the preceding response contains
a question mark, and `agent_response_matches` adds a regex the response must
also match. A turn whose gate is unmet stops the conversation and is recorded as
`agent_did_not_ask` or `agent_response_mismatch` — a real result about the
agent, which is usually the point of scripting the exchange.

## Starting in plan mode

A real session often begins in the harness's plan mode: the agent reads and
plans but cannot edit, presents a plan, and implements only once the person
approves it. An eval declares that shape with `plan_mode`:

```json
{
  "id": "add-request-caching",
  "prompt": "Requests to the pricing API are slow. Can you add caching?",
  "expected_output": "A working cache with the pricing endpoint under 100ms.",
  "plan_mode": true,
  "responder": { "type": "llm" }
}
```

The session runs in two phases, in one native session:

1. **Planning.** The opening prompt is dispatched in the harness's native plan
   mode, so the agent explores read-only. If it asks a question, the responder
   answers it and the session stays in plan mode.
2. **Implementation.** Once the agent has presented its plan, the runner
   approves it with one fixed message and resumes the same session in act
   mode. The message is always `The plan is approved. Implement it now.` From
   there the eval's `turns` or `responder` proceed exactly as they would
   without plan mode.

The approval is fixed rather than judged, so the transition is identical in
every run and both arms: what varies between conditions is the plan and the
implementation, never how the runner reacted to them.

### How the runner knows the plan is ready

Two signals, tried in this order:

| Signal | When it applies |
| --- | --- |
| `plan_file` | The harness writes the plan it presents to a file, and its descriptor declares where (`[plan_mode.plan_file]`). A planning round that wrote one has presented its plan, and the file's content is the plan. Claude Code writes to `~/.claude/plans`. |
| `responder` | The eval declares a `responder`, which is told the agent is planning. Its `done` verdict means the plan is ready, and the agent's last message is the plan. |

A harness that writes no plan file needs the responder to decide, so `run`
rejects a plan-mode eval there unless it declares one. With a plan file the
responder is optional; if the agent never writes one and there is no responder
to ask, the run stops with `plan_not_presented`.

`eval-magic harness list` shows `plan-mode` for every harness that can start a
session in plan mode. `run` rejects a plan-mode eval on one that cannot, before
any environment is built.

### What is recorded

- `outputs/plan.md` holds the approved plan, and the judge evidence bundle
  renders it in a section of its own.
- Every user message in `conversation.json` carries `mode` (`plan` or `act`),
  and the approval turn carries `origin.runner: plan_approval`.
- `conversation.json`'s `plan` names the round the plan was presented in, the
  round the approval opened, and which signal fired.
- A responder's `max_turns` is one budget across both phases: planning-phase
  answers count toward it.
- A plan-mode run whose session never left the planning phase — whatever
  stopped it — is counted per condition in `benchmark.json`'s
  `validity_warnings`, because it never attempted the task.
- An agent that tries to edit while planning is refused by the mode itself.
  That refusal is recorded in `permission-denials.json` as behavioral evidence
  and marked `plan_mode_attributed`, but it raises no validity warning: the
  mode refusing a write is the mode working.

Where the harness writes its plan file, the write guard and the stray-write
audit allow that root beside the task environment. The plan file lands where
the harness puts it in any session; `plan.md` is the copy the judge reads.
