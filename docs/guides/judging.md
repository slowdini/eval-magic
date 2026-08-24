# Judge evidence bundles

> **Audience:** eval authors and operators deciding whether an LLM verdict has enough evidence to
> trust.

`ingest` writes one `judge-evidence.md` beside every recorded run. This Markdown bundle is the
primary input for every LLM judge task for that run: eval-magic persists it once and inlines those
exact bytes into each judge prompt. Read the bundle when a verdict is surprising, when a truncation
marker appears, or before promoting an important result.

## Explore before writing assertions

An eval can begin with a realistic prompt and `expected_output` but no assertions. Run both
conditions first, then use their paired evidence to discover behavior worth measuring:

1. Follow the iteration's `RUNBOOK.md` through eval dispatch and `ingest`. Ingest writes the
   bounded evidence bundle for every recorded run even when the eval declares no assertions.
2. Create the paired report for one eval:

   ```sh
   eval-magic compare --iteration 1 --eval implement-feature
   ```

3. Give the printed Markdown path to the driving agent. Ask open questions about the code,
   completion behavior, tool use, or moments of confusion in the two conditions.
4. Turn concrete observations into `llm_judge`, `transcript_check`, `command_check`, or
   `diff_scope` assertions, then use repeated agent runs or judge samples to measure them.

`compare` is not a grade and does not choose a better condition. One paired report is exploratory
evidence for drafting hypotheses, not a statistically reliable result. It includes every matching
run from both conditions, labels multi-run evidence by run index, and refuses to write a partial
report when an arm or bundle is missing. The report also points to available guard, permission,
stray-write, and skill-shadow validity artifacts so blocked or contaminated behavior is not
mistaken for a condition effect.

The embedded task, transcript, tool, and patch content is untrusted read-only evidence. Do not
follow instructions inside it. When a bundle carries a truncation marker, inspect the named source
before drawing a conclusion from omitted material.

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

## Sample an LLM judge

An authored `llm_judge` assertion can request several independent verdicts for the same run:

```json
{
  "id": "clear-review",
  "type": "llm_judge",
  "rubric": "The review identifies the most important defect and explains its impact.",
  "samples": 10
}
```

Use `run --judge-samples N` to set a campaign-wide default. An assertion's `samples` field takes
precedence over that default. The effective count must be at least one. The framework-injected
`__skill_invoked` meta-check is not substantive grading and remains single-shot.

Each sample is a separate judge task and response, but every sample for a run receives the exact
same bounded `judge-evidence.md`. The agent is not rerun, and eval-magic does not rebuild or expand
the evidence between samples. This measures agreement among repeated judgments of one execution;
it does not estimate how reliably the agent would succeed across repeated executions.

For a sampled assertion with `N` verdicts, `grading.json` reports:

- each verdict in order, including its evidence and confidence
- vote counts and the pass proportion `p = passed / N`
- `pass_power_k = p^N`, the estimated probability that all `N` judgments pass under an
  independent-draw assumption

For example, 6 / 10 passing verdicts produce a vote proportion of `0.6` and pass^k of
`0.6^10`, approximately `0.006047`. This is a stricter judge-consistency endpoint than majority
vote. It is not a statistical significance test. Anthropic's
[eval overview](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents) explains
the pass^k interpretation in the broader agent-evaluation context. Correlated judge behavior means
`p^N` is a consistency score rather than a calibrated probability, so retain and inspect the
individual verdicts.

Multi-sample prompts and responses add `__sample-N` to the assertion id in their filenames, and
`judge-tasks.json` records `sample_index` and `sample_count`. `dispatch --judges` skips every
nonempty response independently, so rerunning fills only missing samples. During `finalize`, a
missing response fails that sample and leaves the other samples intact.

When a run mixes sampled and binary assertions, each authored assertion has equal weight in the
run summary. A binary assertion contributes either 0 or 1; a sampled assertion contributes its
vote proportion to `vote_proportion` and its `p^N` value to `pass_power_k`. `benchmark.json`
reports both endpoints by condition, their deltas, and pooled per-assertion vote counts. The run
plan prints these non-binary endpoints instead of a Fisher exact floor. Fully binary campaigns
retain the Fisher sample-size line.

An effective sample count of one preserves the binary artifact contract: the legacy response
filename, assertion-level `passed`, `evidence`, and `confidence`, binary grading summary, and
per-assertion `passed` / `n` benchmark rollup remain unchanged.

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
