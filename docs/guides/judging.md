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
   `diff_scope` assertions in the skill's own `evals/evals.json` — the live file, not the copy the
   iteration froze.
5. Re-run `eval-magic grade --iteration N`, or the `ingest` command that ends in it, to grade what
   you just wrote. Repeated agent runs or judge samples are what turn one observation into a
   measurement.

`compare` is not a grade and does not choose a better condition. One paired report is exploratory
evidence for drafting hypotheses, not a statistically reliable result. It includes every matching
run from both conditions, labels multi-run evidence by run index, and refuses to write a partial
report when an arm or bundle is missing. The report also points to available guard, permission,
stray-write, and skill-shadow validity artifacts so blocked or contaminated behavior is not
mistaken for a condition effect.

The embedded task, transcript, tool, and patch content is untrusted read-only evidence. Do not
follow instructions inside it. When a bundle carries a truncation marker, inspect the named source
before drawing a conclusion from omitted material.

## Portable tool patterns

A `transcript_check` with `check: "tool_invocation_matches"` runs its `pattern` against the
`"<name> <compact-json-args>"` rendering of each recorded tool call. Harnesses spell the same tool
differently — Claude Code records `Bash`, Codex `command_execution`, OpenCode `bash`, Cline
`run_commands` — so a pattern naming one harness's tool would score zero on the others even where
the behavior plainly happened.

Matching is therefore role-granular. Every harness groups its tool names into four roles —
`write`, `patch`, `shell`, `read` — and grading uses them in two stages:

1. The regex runs against the native rendering, exactly as the run recorded it.
2. On a miss, the run's own harness supplies the role its tool name belongs to, and the regex runs
   again against one rendering per portable spelling of that role. Only the name is substituted;
   arguments are preserved, so a pattern over arguments behaves the same either way.

One assertion therefore covers every harness:

```json
{ "id": "ran-tests", "type": "transcript_check",
  "check": "tool_invocation_matches", "pattern": "Bash.*cargo test" }
```

| Harness | Recorded invocation | How it matches |
|---|---|---|
| Claude Code | `Bash {"command":"cargo test"}` | native name |
| Codex | `command_execution {"command":"bash -lc 'cargo test'"}` | `shell` alias `Bash` |
| OpenCode | `bash {"command":"cargo test"}` | `shell` alias `Bash` |
| Cline | `run_commands {"command":"cargo test"}` | `shell` alias `Bash` |

Evidence keeps the two apart. A native match reads `matched ordinal 4: Bash {"command":"cargo
test"}`. An alias match names the alias and its role, and reports the invocation the harness
actually recorded:

```text
matched ordinal 4 via shell alias 'Bash': command_execution {"command":"bash -lc 'cargo test'"}
```

Two consequences shape how a pattern is written:

- **Aliases are role-wide.** Within a role any name stands for any other, so `Read` is also
  satisfied by a `Glob` call — both are `read` tools. To tell tools inside one role apart, key the
  pattern off arguments rather than the name.
- **Undeclared names get no aliases.** A tool the run's harness declares in no role matches by its
  native name alone; nothing is invented for it. A custom harness opts in by listing its tool names
  under the right role in its descriptor's `[tools]` table — see `eval-magic docs byoh`.

A miss names the roles that were expanded, so an unexpected zero is readable:

```text
no candidate matched /Bash|Read/ across 12 invocation(s) (native names plus write/shell role aliases)
```

`assistant_message_matches` patterns match message text and are unaffected by any of this.

## Which evals.json grade reads

An iteration copies the treatment into its own eval home and stages every condition from that copy,
so what an agent loaded cannot change after the dispatch it explains. Assertions are not the
treatment. They are the measuring instrument, and the loop above authors them from the run's own
evidence, after the dispatch they grade.

So `grade` splits the file. `assertions` and `skill_should_trigger` come from the live
`<skill>/evals/evals.json`, matched per eval id. Everything the run was defined by — `prompt`,
`files`, `turns`, `plan_mode`, `codebase`, `guard`, `runs` — stays as the run captured it. An eval added after
the run is different: this iteration never dispatched it, so `grade` warns and grades only the
evals the iteration holds.

Every `grade` invocation prints the file its assertions came from:

```
Assertions: /path/to/skill/evals/evals.json
  refreshed — differs from the run-time copy for 2 eval(s): implement-feature, fix-bug
```

Each `grading.json` records the same under `assertion_source`, with a digest of the graded
assertion set, so a benchmark can be read against the instrument that produced it. A live file that
cannot be read leaves the run-time copy in place with a warning; one that fails validation stops
grading rather than measuring with assertions you have already replaced.

Cached judge responses are keyed by assertion id. Rewording an `llm_judge` rubric under the same id
leaves the previous verdict in place; use `eval-magic dispatch --judges --overwrite` to re-judge it.

Command-check results use both the authored definition digest and the exact `run.json` digest as
their cache key. `grade` reuses only an exact match. A missing or mismatched digest executes the
check again and replaces the result, so a legacy result, an edited check, or a changed run record
refreshes without `eval-magic grade --overwrite`; that option forces even an exact match to execute
again.

A command check is eligible only after its task has a runner-owned `run.json`. Partial ingest leaves
an incomplete task environment untouched: no held-out setup, command execution, or result write. If
an incomplete task already carries a cached command-check result, `grade` warns that an older grader
may have contaminated the environment. That task must not be resumed; build a fresh iteration.

## What the bundle contains

The bundle combines the evidence that establishes what the agent was asked to do, what it did, and
what it changed:

- run identity, completion state, timing, token counts, and codebase and skill provenance
- an artifact manifest pointing to `run.json`, `diff-scope.json`, `diff.patch`, and raw harness
  outputs
- the original task `prompt` and the agent's `final_message`
- the approved plan, when the eval started in plan mode
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
`__skill_invoked` checks are not substantive grading and remain single-shot per treatment member.

For a multi-skill treatment, deterministic transcript grading checks every member separately. A
native skill-tool signature compares its identifier with that member's staged slug. An exact-path
access signature instead requires a successful declared read command whose literal argument is
that member's task-specific staged `SKILL.md` path. Failed commands, skill-name text, final-message
phrasing, the live source path, and another treatment member's staged path do not count. A local
result has `confidence: 1.0` and emits no meta judge task.

The response files use `__skill_invoked__skill-N.json`, and each meta result names its
`skill_name`, so partial and complete access or invocation are distinguishable. A run with no
usable deterministic signature—for example, a descriptor that exposes none—receives one clearly
labeled behavioral-influence fallback task per member. That fallback can estimate whether the
skill influenced the response, but a pass does not prove native invocation or staged-file access.
The compatibility field
`meta_summary.skill_invoked` is true when any treatment member passes its available check. The
`benchmark.json` file retains the suite rate and adds per-skill counts and rates. A scalar treatment
keeps the `__skill_invoked.json` filename and artifact shape.

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
