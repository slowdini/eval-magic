# Isolating dispatches from live skill sources

> **Audience:** operators whose run printed a skill-shadow warning or whose
> `plugin-shadow.json` or `benchmark.json` reports a live copy of an eval skill.

Use this guide after eval-magic reports a discoverable live skill. It explains how to exclude that
source from eval-agent dispatches and how to verify the result. Normal task-repository isolation and
the write guard are separate concerns documented by `eval-magic run --help`.

## Why a live copy matters

An eval compares the same task with and without a skill, or with two revisions. If the harness can
discover another copy of the subject skill in both arms, the control arm is not skill-free and the
delta is invalid.

Sibling collisions have two outcomes:

- A sibling visible in both arms of every group is a warning because the comparison remains
  symmetric but differs from a clean environment.
- A sibling visible in only one arm is comparison-invalid because its effect cannot be separated
  from the skill under test.

The preflight reports two source classes in schema-v3 `plugin-shadow.json`:

- `operator-environment` — global skills, enabled plugins, and other sources inherited from the
  machine running eval-magic.
- `codebase-sourced` — matching project-local skills preserved from the task codebase.

Transcript evidence can later show what a dispatch loaded. Eval-magic does not parse shell
templates to infer that a flag or environment variable isolates the process.

Apply the remedy to **every eval-agent command**, including every resumed turn of a scripted eval.
Isolating only the first round allows the live copy to return on the next round. Judge commands do
not need isolation because they receive a rubric and transcript rather than the task itself.

## Isolate Claude Code

Choose one remedy:

| Remedy | Hides | Leaves visible | Caveat |
| --- | --- | --- | --- |
| `--setting-sources project,local` | User-scope plugins and `~/.claude/skills` | Project and local settings, including staged skills and the guard | Authentication is unchanged. |
| Disable one entry in `enabledPlugins` | The named plugin | Other plugins and global skills | The setting must be in a source loaded by the dispatch. |
| A fresh `CLAUDE_CONFIG_DIR` | Installed plugins and global skills | Project-local staged content | OAuth state may not follow the relocated directory; use an API key or authenticate it. |

`--setting-sources project,local` is the usual first choice because it changes one dispatch without
editing global configuration. Project-local staged skills still load under every remedy.

## Isolate Codex

Choose the remedy that matches each reported source:

| Remedy | Hides | Leaves visible | Caveat |
| --- | --- | --- | --- |
| `codex --disable plugins ... exec` | Skills from enabled plugins | Repository, user, and admin skill directories | `--disable plugins` is global and must appear before `exec`. |
| Move or rename the conflicting directory | That direct skill source | Every other source | Required for repository and admin skills. |
| A clean `HOME` | `$HOME/.agents/skills` | `CODEX_HOME` plugins and repository/admin skills | Preserve `CODEX_HOME` when the dispatch needs the installed configuration. |

Codex also has bundled system skills without a stable enumeration surface. Check possible name
collisions with them separately.

## Isolate OpenCode

OpenCode can load skills installed for other harnesses, including `~/.claude/skills` and
`~/.agents/skills`.

| Remedy | Hides | Leaves visible |
| --- | --- | --- |
| `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1` | Global and project `.claude` roots | `.agents` and `.opencode` roots |
| `OPENCODE_DISABLE_EXTERNAL_SKILLS=1` | `.claude` and `.agents` roots | `.opencode` roots |
| Move or rename the conflicting directory | That direct source | Every other source |

The preflight cannot enumerate config-declared `skills.paths`, remote `skills.urls`, or the singular
`.opencode/skill/` directory. Check those sources when the project uses them.

## Declare isolation honestly

After excluding every reported source from every initial and resumed eval-agent command, record the
result in a descriptor layer:

```toml
label = "claude-code"

[shadow]
isolates_live_sources = true
```

The declaration covers only `operator-environment` findings. It does not claim that skills sourced
from the task codebase are isolated. Use `codebase.exclude_skill_sources: true` for that separate
policy when project skills should not participate; see `eval-magic docs codebase`.

The declaration does not disable detection. `plugin-shadow.json` retains every source and its
intrinsic severity as provenance. `run` presents operator-environment findings as informational,
and `aggregate` omits those warnings only while no transcript evidence contradicts the declaration.
Codebase-sourced findings remain warnings regardless of this descriptor setting.

Do not set it when:

- Any reported source remains discoverable.
- A scripted resume command lacks the remedy.
- Codex uses `--disable plugins` but the report also names a direct skill directory.
- OpenCode uses an external-skill switch but the report names an `.opencode` source.
- You have not verified every rendered eval-agent command.

Partial isolation does not qualify.

## Verify the result

From the iteration directory, read `plugin-shadow.json` after `ingest`:

```sh
jq -r '.findings[] | "\(.skill_name): \(.resolved_severity // "not verified")"' \
  plugin-shadow.json
```

The outcomes mean:

- `isolated`: every expected cell reported a roster and none loaded the source.
- `comparison-invalid` or `warning`: a dispatch loaded the source or the evidence could not settle
  the finding.
- No `resolved_severity`: the harness cannot report a roster or ingest has not run.

Refuting a finding requires evidence from every expected cell. One confirmed load is enough to keep
the warning. A missing transcript never proves isolation.

Claude Code stream JSON includes a session-opening `{"type":"system","subtype":"init"}` record.
From one task's `outputs/` directory, inspect it directly:

```sh
jq 'select(.subtype == "init") | {plugins, skills}' claude-events.jsonl
```

An empty array is positive evidence that no entries of that type loaded. Check the specific runtime
ID rather than the total list length because staged and bundled skills remain present. Scripted runs
produce an init record under each `turn-N/`; inspect a resumed turn as well.

Codex and OpenCode captures do not provide the equivalent roster used by eval-magic. Verify those
harnesses by checking the eval-agent command `dispatch-manifest.md` says the runner will spawn,
then use `isolates_live_sources` to record the operator assertion.

### `claude plugin list` does not prove isolation

`claude plugin list` reports installed plugins, not what one dispatch loaded. It does not accept the
dispatch's setting-source selection. A plugin can appear there and remain absent from the dispatch,
or the reverse. Use the dispatch's init event.

## The skill under test is a copy

Every skill an eval stages is copied into the eval home before any dispatch runs, and
each condition stages from that copy. Nothing the agent can reach is read from your own
skill directory, so editing a skill mid-campaign cannot change what a prepared iteration
measures.

The copy is the working tree as it sits on disk, not a checkout of a commit —
evaluating an uncommitted revision is the ordinary case, and in a `--mode revision` run
the edit under test is uncommitted by definition. What the run measured is recorded rather than inferred, in
`conditions.json`, each `run.json`, `benchmark.json`, and the `BASELINE.md` written by
`promote-baseline`:

```sh
jq '.skill_source' conditions.json
```

`dirty: true` means the recorded revision alone does not identify what ran. Commit the
skill before a run whose result you intend to publish.

Sibling skills staged by `--skill-dir` are copied the same way, and the roster is
captured once when the run resolves. The `siblings` field names exactly what every
environment received.

The eval home sits outside the skill's own repository: under `$XDG_DATA_HOME/eval-magic`
(or `~/.local/share/eval-magic`), in a directory named for the skill directory it serves.
`run` prints the path it chose, and every command it suggests carries `--workspace-dir`,
so there is nothing to remember. `EVAL_MAGIC_WORKSPACE_DIR` moves the default;
`--workspace-dir` overrides both.

Copying does not remove the live directory from the machine, so a dispatch can still read
it by absolute path. `detect-stray-writes` reports that as a live-source read, and
`aggregate` carries it into `validity_warnings` for the same reason a discoverable
plugin copy is carried there: the arm may not be comparing what it claims to.

## The task repository is a separate boundary

Skill-source isolation is about what a dispatch can *load*. The task repository is about what it can
*reach*: every dispatch runs in its own private environment, a Git repository with no remotes and
hooks disabled, marked with `refs/eval-magic/baseline` at the state the agent started from. That
holds whether the environment was built from fixture files or from a sourced codebase — see
`eval-magic docs codebase`.

The two are independent. An environment can be a faithfully isolated repository while the dispatch
still loads a live skill source, and a shadowed skill is not made safe by the repository boundary.

## When a source cannot be isolated

Do not declare isolation. Retain the validity warning as the record of a known threat. A symmetric
sibling collision can still support a qualified comparison, but a subject collision or asymmetric
sibling collision requires another run after the source is excluded.
