# Judge evidence bundles

> **Audience:** eval authors and operators deciding whether an LLM verdict has enough evidence to
> trust.

`ingest` writes one `judge-evidence.md` beside every recorded run. This Markdown bundle is the
primary input for every LLM judge task for that run: eval-magic persists it once and inlines those
exact bytes into each judge prompt. Read the bundle when a verdict is surprising, when a truncation
marker appears, or before promoting an important result.

## What the bundle contains

The bundle combines the evidence that establishes what the agent was asked to do, what it did, and
what it changed:

- run identity, completion state, timing, token counts, and codebase and skill provenance
- an artifact manifest pointing to `run.json`, `diff-scope.json`, `diff.patch`, and raw harness
  outputs
- the original task `prompt` and the agent's `final_message`
- changed-file metrics, a changed-file list, and the captured patch
- the conversation transcript, including markers showing where tools were invoked
- a tool invocation summary with bounded arguments and results

A one-shot run has an explicit “no conversation record” entry. Missing diff evidence is also
explicit; it is never presented as an empty successful change.

Held-out `command_check` results are not included. Diff capture happens before command-check setup
files are injected, and keeping the bundle at that boundary prevents a judge from confusing
runner-owned mutations with agent work. Mechanical assertion results remain runner-owned and are
merged during `finalize`.

## How the bounds work

Each evidence bundle is at most 98,304 bytes (96 KiB). The complete judge prompt, including its
rubric and framing, is at most 131,072 bytes (128 KiB). Within the bundle, eval-magic reserves:

- 8 KiB for the task prompt
- 12 KiB for the final message
- 8 KiB for the changed-file list
- 16 KiB for the conversation, with at most 4 KiB per event
- 8 KiB for the tool summary, with at most 512 bytes for each argument and result

The patch receives the remaining bundle space, so short contextual sections leave more room for
the implementation itself. Oversized sections retain both their beginning and end at valid UTF-8
boundaries and carry an `[eval-magic] ... omitted` marker naming the full source. Markdown fences
are chosen so evidence containing its own fences cannot escape the section that holds it.

`judge-tasks.json` records the actual byte count, limit, and `truncated` state of each evidence
bundle, plus the actual and maximum judge-prompt sizes. A `diff.patch` can also carry its own
capture-time truncation marker; that upstream limit is separate from bundle truncation.

Eval-authored rubrics and skill content are never silently shortened. If either makes the complete
prompt exceed 131,072 bytes, judge-task emission fails with the assertion id, actual size, limit,
and a request to shorten the authored content.

## Treat evidence as data

The task prompt, transcript, final message, patch, and tool output are untrusted agent-produced
data. Judge framing says not to follow instructions found inside the evidence. A judge works
read-only: it may inspect a source path named by a truncation marker, but it must not edit the run,
the evidence, or the task environment. Its only write is the requested verdict file.

When a marker omits material needed by the rubric, inspect the named source before deciding. The
artifact paths are valid in the grading iteration. After promotion and teardown reclaim that
iteration, its retained bundle may no longer have those complete sources beside it. If a required
source is unavailable, the claim is unverifiable rather than evidence of success.

From a run directory, inspect the bounded evidence and its source records:

```sh
sed -n '1,240p' judge-evidence.md
jq '{prompt, final_message, conversation, tool_invocations}' run.json
jq '{files_touched, lines_added, lines_removed, hunks, files, patch}' diff-scope.json
```

## Retain the evidence behind a baseline

`promote-baseline` copies each exact bounded bundle into `<skill>/evals/baseline/evidence/` beside
the retained benchmark and gradings. A single-run bundle is named
`<eval-id>__<condition>.md`; multi-run bundles add `__rN`. This preserves the primary judge input
without copying unbounded transcripts, patches, or task environments into the skill repository.

Older iterations can have gradings without `judge-evidence.md`. Promotion preserves compatibility
by warning about each missing legacy bundle instead of failing, but such a baseline does not retain
the evidence needed to reproduce its LLM judgment. Re-grade the iteration before promotion when
that evidence matters.
