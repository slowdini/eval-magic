# Sourcing a codebase into a task environment

> **Audience:** Eval authors choosing, pinning, and verifying a project for coding tasks.

Every eval runs against a project codebase. Declare a `codebase` in `evals.json` and every
`(eval, condition, run)` environment is built from a checkout of it — with history, on a branch,
ready for the agent under test to work in.

This matters for anything you cannot judge from a toy problem. Whether a skill makes an agent's
code *better* is not answerable when the task is small enough that any model succeeds.

## Choose a source during `init`

With no codebase option, `eval-magic init` uses the Weeknight example project at its pinned
baseline:

```sh
eval-magic init
```

The generated `evals/evals.json` contains this `codebase` value:

```json
{
  "url": "https://github.com/slowdini/eval-magic-fixture",
  "ref": "b6d269c1cdedf7cadb53bacc41acaf5f2cdbe03f"
}
```

Choose another Git source by providing its URL and ref together:

```sh
eval-magic init \
  --codebase-url https://github.com/slowdini/eval-magic-fixture \
  --codebase-ref b6d269c1cdedf7cadb53bacc41acaf5f2cdbe03f
```

`init` records those values without contacting the remote. `run` resolves the source and fails
before provisioning environments if the repository or ref is unavailable.

For local work, name a directory already on disk or use the invocation directory itself:

```sh
eval-magic init --codebase-path .
eval-magic init --codebase-cwd
```

A relative `--codebase-path` resolves from the directory where you invoke `init`. Relative path
inputs and `--codebase-cwd` are written relative to the generated `evals/` directory. An absolute
`--codebase-path` remains absolute. Local sources are convenient for iteration but carry the
portability limits described under "A `path` source is not reproducible elsewhere."

The URL/ref, local path, and current-directory modes are mutually exclusive. The chosen source is
written into the eval file, so the committed configuration records which project the suite uses.

## Choose the project scale

### Start with Weeknight

[Weeknight](https://github.com/slowdini/eval-magic-fixture) is a React and TypeScript meal-planning
web app. It has multiple routes, browser persistence, ingredient aggregation, tests, linting, and a
production build without a backend or external service. Use it for a first full-codebase eval or for
tasks where a compact project makes the agent's decisions easy to inspect.

Suitable tasks include changing planner validation, extending the recipe filters, migrating stored
state, fixing shopping-list aggregation, or improving an interaction with focused tests. Pin the
project commit in the eval file even when a later revision exists; change the ref as a
deliberate eval-suite revision.

### Use eval-magic as a complex project

Use [eval-magic](https://github.com/slowdini/eval-magic) when the skill needs a larger codebase with
cross-module Rust behavior, schemas, generated artifacts, integration tests, and repository-level
contributor instructions. This project is appropriate when navigating and preserving those
contracts is part of what the eval should measure.

This command scaffolds a pinned eval-magic source:

```sh
eval-magic init \
  --codebase-url https://github.com/slowdini/eval-magic \
  --codebase-ref e30a5091c844e07f3aa664413aa7735a11b0a52a
```

The larger repository increases preparation, dispatch, and review work. Prefer Weeknight unless the
task genuinely needs the extra architectural surface. Project instructions and project-local skill
sources remain part of either project unless the eval opts out as described below.

## Declare one

A git repository, with an explicit ref:

```json
{
  "skill_name": "working-with-tdd",
  "codebase": { "url": "https://github.com/slowdini/example-project", "ref": "v1.4.0" },
  "evals": [
    { "id": "add-a-feature", "prompt": "...", "expected_output": "..." }
  ]
}
```

Or a directory on this machine:

```json
{ "codebase": { "path": "../../projects/legacy-service" } }
```

A relative `path` resolves against the directory holding `evals.json`, so a committed config means
the same thing in every clone of the skill. Unlike `files_root`, it may be absolute or point
outside the skill tree — that is the point of it.

The config-level `codebase` is a default. Any eval can override it:

```json
{
  "codebase": { "url": "https://github.com/slowdini/example-project", "ref": "main" },
  "evals": [
    { "id": "small-fix", "prompt": "...", "expected_output": "..." },
    { "id": "big-refactor", "prompt": "...", "expected_output": "...",
      "codebase": { "path": "/srv/projects/monolith" } }
  ]
}
```

## `ref` is required

A git source must name a branch, tag, or full commit SHA. The runner resolves it and records the
commit, so a report says which tree it measured. An eval tracking whatever `main` happened to be
could not be re-run against the state it reported on, which is the point of recording provenance at
all.

Resolution happens before any environment is created. An unreachable repository or a ref that does
not exist fails the run while it has still built nothing.

## Project config and skill sources

The sourced tree is preserved by default, including harness instructions, settings, plugins, and
project-local skills. For example, `CLAUDE.md`, `AGENTS.md`, `.claude/settings.json`, and
`.opencode/settings.json` remain visible in every comparison arm.

Preserving project skills can contaminate a comparison when the codebase provides the
subject or one of its staged siblings. `run` records those matches in `plugin-shadow.json` with
`class: "codebase-sourced"`, separately from `class: "operator-environment"` findings caused by
global skills or installed plugins. Subject collisions are comparison-invalid; sibling collisions
follow the symmetric/asymmetric rules in `eval-magic docs isolation`.

Opt an eval out of only the harness-discoverable project skill roots when the codebase's skills are
not part of the task being measured:

```json
{
  "codebase": {
    "url": "https://github.com/slowdini/example-project",
    "ref": "v1.4.0",
    "exclude_skill_sources": true
  }
}
```

The default is `false`. When set to `true`, eval-magic moves every project skill root declared by
the selected harness out of each task environment before staging. It applies equally to both arms,
every repetition, revision mode, and `--no-stage`. Root instruction files and other harness config
remain in place. OpenCode, for example, excludes `.opencode/skills`, `.claude/skills`, and
`.agents/skills` because its descriptor declares all three discovery roots. For a BYOH descriptor
with no project skill roots, the setting is recorded and makes no filesystem change.

Generated staging slugs are collision-safe: if the codebase owns that exact directory, the
runner backs it up, stages the evaluated copy for that arm, and restores the original during
cleanup. An explicit `--stage-name` remains stricter and refuses to clobber an occupied directory.

The effective `exclude_skill_sources` value is recorded with each codebase in `conditions.json`,
every task in `dispatch.json`, every `run.json`, `benchmark.json`, and promoted `BASELINE.md`.

## Framework files stay out of the project's own tooling

Staged skills live *inside* the task repository, under the harness's skills dir. A project whose
lint or format step globs the whole tree would otherwise report eval-magic's own artifacts as
project failures — and only in the arm that stages a skill, because the control arm has no staged
skills to find. That biases every `command_check` running the project's checks, and it hands the
agent under test a red check listing files it did not create and must not touch.

So `run` writes a delimited block into the project's own ignore files, naming the paths the runner
placed:

```
# >>> eval-magic framework files >>>
# Staged by `eval-magic run` so this project's own tooling does not report them.
# See `eval-magic docs codebase`.
/.eval-magic-outputs/
/.claude/skills/
/.claude/settings.local.json
# <<< eval-magic framework files <<<
```

The paths come from the selected harness descriptor — its `skills_dir` and the file its write guard
stages — plus the framework outputs directory, so a BYOH harness gets its own paths without
configuring anything. The block is written into **every** environment: both arms, every repetition,
revision mode, `--no-stage`, and `--dry-run`. An entry present in one arm only would trade one
asymmetry for another.

Which ignore files get the block is detected from the codebase's own tooling:

| Detected | Ignore file | Created when the project has none |
| --- | --- | --- |
| Prettier | `.prettierignore` | yes |
| ESLint | `.eslintignore` | no — ESLint 9's flat config no longer reads it |
| Stylelint | `.stylelintignore` | yes |
| markdownlint | `.markdownlintignore` | yes |
| Docker | `.dockerignore` | yes |

Detection reads config-file markers and `package.json` dependencies anywhere in the tree. The list
is short because the ignore-file convention is: Python, Rust, and Go formatters have no ignore file
and already skip dot-directories, so they never see the staged skills in the first place.

Name the ignore files yourself when the project's are somewhere detection will not look, or when
you want none at all:

```json
{
  "codebase": {
    "url": "https://github.com/slowdini/example-project",
    "ref": "v1.4.0",
    "ignore_files": ["tooling/.prettierignore"]
  }
}
```

A declared list replaces detection rather than extending it, and each path is created if missing.
Paths are relative to the environment root and may not escape it. `"ignore_files": []` opts out
entirely, leaving every ignore file exactly as the codebase wrote it.

`.gitignore` is never a target. The baseline commit force-adds harness config dirs, so an entry
there would hide nothing from Git — but it *would* hide the staged skills from every
`.gitignore`-aware tool the agent uses, such as `rg`, which would damage the treatment arm instead
of protecting it.

Unlike `exclude_skill_sources`, no artifact records this: the written file is committed into each
environment's baseline, so `git show refs/eval-magic/baseline:.prettierignore` inside any task
environment is the evidence, per arm and per run. `run` also prints the files it wrote.

## What the environment contains

Each dispatch gets its own private environment holding:

- the codebase, checked out at the resolved commit, with its history intact
- no remotes — nothing in the environment can reach or push to the source it came from
- hooks disabled, and a fixed committer identity for the runner's own commit
- the branch the codebase itself was on: the branch a `ref` names, or the repository's default
  branch when the ref is a tag or a SHA
- `refs/eval-magic/baseline`, marking the state the agent started from

Every selected eval must resolve an effective codebase, either from the top-level default or an
eval-level override. Validation rejects a configuration that supplies neither.

## The baseline ref is what the run is measured against

Nothing writes into an environment after that ref is written, so it names exactly what the agent
started from — and everything the agent did is the difference from it.

During `ingest`, Git measures that difference. Each run gets:

- `diff-scope.json` — `files_touched`, `lines_added`, `lines_removed`, and `hunks`, plus the list of
  changed files with a status of `added`, `modified`, or `deleted`
- `diff.patch` — the diff itself, which is the evidence a judge reads to answer whether the work was
  any good. It always exists; for a run that changed nothing it is empty. A diff past the capture
  cap is cut at a line boundary and carries a marker saying so, and `patch.truncated` in
  `diff-scope.json` records it.
- `judge-evidence.md` — the bounded grading input that combines this diff with the task, completion
  state, conversation, and tool summary. See `eval-magic docs judging` for its limits, trust
  boundary, and retained-baseline behavior.

What counts is what Git counts, under the same rules the baseline commit was built under:

- The codebase's own `.gitignore` holds, so a run that compiles does not report its build output as
  thousands of touched files.
- Overlay files and staged skills count even when the codebase ignores their paths — they are
  committed into the baseline regardless, so a change to one is always visible.
- Framework artifacts under `.eval-magic-outputs/` never count.
- A nested repository's internals never count: Git tracks no path with a `.git` component.
- A rename counts as two touched files, one created and one deleted.
- A binary file counts as one touched file, contributing no lines.

A `diff_scope` assertion gates `max_files_touched`, `max_lines_changed` (added plus removed), or
both, against exactly these numbers.

## One checkout per iteration

Every environment a run provisions — each `(eval, condition, run)` cell — is built from one cached
checkout per distinct codebase, materialized once per iteration under `iteration-N/.codebase/`
when the run prepares.
Environments are provisioned from that cache with `git clone --local`: Git hard-links the object
store instead of copying it and checks out a fresh working tree, so `--runs 10` against a large
repository costs one clone plus a working tree per environment, not a full copy of the tree and
its history per environment.

Shared objects are content-addressed and immutable — Git never rewrites an object once written —
so each environment is still an independent working tree with an independent history. Commits,
branches, and edits in one environment are invisible to the others and to the cache.

Where hard-linking is unavailable (the cache and the environments on different filesystems) or the
source carries no Git history to clone, environments fall back to a plain copy of the cache. The
result is the same tree, provisioned more slowly.

## `files` is an overlay

`files` and `files_root` still work, and are applied *on top* of the codebase at their declared
paths. Seeding a task-specific file into a real project is the common case:

```json
{
  "id": "add-a-feature",
  "prompt": "Implement what docs/TASK.md describes.",
  "expected_output": "the feature, with tests",
  "files": ["docs/TASK.md"]
}
```

An overlay file replaces a codebase file at the same path.

The baseline the runner commits respects the codebase's `.gitignore`, so ignored build output stays
out of it. Overlay files and staged skills are committed regardless of what the codebase ignores —
which is also what keeps them inside every later measurement.

## A `path` source is not reproducible elsewhere

Someone reading your published results cannot resolve `../../projects/legacy-service`. Their machine
has that directory somewhere else, or not at all. Nothing can fix that, so the artifacts label it:
the record carries `host_local: true`, the run prints a warning, and the `BASELINE.md` row says so.

Where the directory is itself a Git repository, its `origin` URL and the resolved commit are
recorded too, and *those* resolve anywhere. Prefer a `url` source for anything you intend to
publish.

A `path` source is materialized as a clean checkout of its committed state. Uncommitted work in the
source directory is not carried into the environment; the run warns when the source is dirty.

## Verify the result

From a prepared iteration directory, inspect one environment:

```sh
cd env-g1-with_skill
git log --oneline | head
git remote -v
git rev-parse refs/eval-magic/baseline HEAD
git status --porcelain
```

`git remote -v` and `git status --porcelain` are both empty, and the two revisions match: the
baseline ref names exactly what the agent started from.

After a dispatch and `ingest`, read what the run produced:

```sh
jq '{files_touched, lines_added, lines_removed, hunks, files, patch}' diff-scope.json
head -50 diff.patch
sed -n '1,240p' judge-evidence.md
```

The same difference, spelled by Git itself, is `git diff refs/eval-magic/baseline` inside the
environment.

The resolved commit and effective skill-source policy appear in `conditions.json`, each `run.json`,
`benchmark.json`, and the `BASELINE.md` written by `promote-baseline` — alongside the skill the run
measured, which is recorded the same way:

```sh
jq '.codebases, .skill_source' conditions.json
```

See `eval-magic docs isolation` for what the skill side of that record means.
