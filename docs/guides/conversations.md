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
`conversation-resume` capability for every harness that has it.

## The responder

```json
{
  "id": "add-request-caching",
  "prompt": "Requests to the pricing API are slow. Can you add caching?",
  "expected_output": "A working cache with the pricing endpoint under 100ms.",
  "responder": { "type": "heuristic", "max_turns": 8 }
}
```

- **`type`** is required. `heuristic` is the only responder today. It is
  deterministic and costs nothing: it reads the agent's message and applies
  fixed rules, with no second model involved.
- **`max_turns`** bounds how many follow-ups the responder may synthesize. The
  opening prompt is not one of them. It defaults to 8.

Every turn the responder produces is recorded in the run's `conversation.json`
with an `origin` naming the rule that produced it, so you can audit whether the
responder distorted the run instead of taking the transcript on trust. The
eval's own opening prompt carries no `origin` — that absence is how you tell an
authored turn from a derived one.

## What the heuristic answers

The heuristic reads the last message of each round as Markdown and answers
exactly one shape of question: **a list of options introduced by a question.**

A list counts as a question when the line directly above it ends with a `?`:

```
Which cache should I use?

- An in-process LRU (Recommended)
- Redis
```

The `?` has to be the last thing said before the options appear. That is what
separates a real question from a closing summary, which is also mostly a
bulleted list and would otherwise be "answered" as though the finished task were
still open.

Given a list, the choice is mechanical:

| The list | Recommendation marked | The answer |
| --- | --- | --- |
| plain (`-`, `*`, `1.`) | yes | the first recommended option |
| plain | no | the first option |
| checkboxes (`- [ ]`) | yes | every recommended option |
| checkboxes | no | nothing — `None of these.` |

Plain lists ask for exactly one choice; checkboxes ask for zero or more. That
syntax is the only signal the heuristic uses to tell them apart.

An option counts as recommended when it carries a standalone `recommended` in
parentheses, brackets, or bold — `(Recommended)`, `[recommended]`,
`**Recommended**` — or when it is a pre-checked box, `- [x]`.

A message that asks more than one question is answered in one turn, numbered in
the order the questions appeared.

## How a conversation ends

| Recorded as | When |
| --- | --- |
| `completed` | The agent's last message asked nothing. It considers the task done, so the run stops rather than burning its remaining turns. |
| `stopped`, `responder_cannot_answer` | The agent asked something with no option list. |
| `stopped`, `max_turns_reached` | The agent was still asking at the bound. |
| `timed_out` | The task outran `dispatch --timeout`. |

A `stopped` conversation is recorded, not failed: `dispatch` exits zero and
`ingest` still records the run. But both responder stops end the conversation
with the task unfinished, so `dispatch` warns about each one by name. Read the
last assistant message before treating such a run as a data point beside a
completed one.

Two properties are worth knowing before you read results:

- The heuristic never guesses. A question it does not recognize stops the run
  instead of inventing an answer, because a fabricated answer would silently
  change what the agent was asked to do.
- It errs toward stopping. A question mark anywhere in an otherwise-finished
  message stops the run rather than calling it complete. That costs a dispatch;
  the alternative — recording a run as complete while the agent was still
  waiting — would cost the result's credibility.

Answering free-form questions needs a model, not rules. That is a separate
responder, and until it ships, `responder_cannot_answer` is where those runs
stop.

## Cross-harness behaviour

The heuristic reads plain Markdown out of the agent's message, so it needs no
per-harness support: any harness that can resume a session can run a responder
eval. Nothing is read from a harness-native question tool, and nothing needs to
be, because a dispatch runs headless with no channel to answer such a tool on.

The shapes above are a contract, not a description of one agent. An agent that
offers options this way is answered; one that phrases them some other way stops
the run. If you are bringing your own harness and its agent asks in a shape the
table does not cover, that is a gap in the table, not in your descriptor.

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
