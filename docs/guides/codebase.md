# Sourcing a codebase into a task environment

An eval's environment can be a real project rather than a handful of fixture files. Declare a
`codebase` in `evals.json` and every `(eval, condition, run)` environment is built from a checkout
of it — with history, on a branch, ready for the agent under test to work in.

This matters for anything you cannot judge from a toy problem. Whether a skill makes an agent's
code *better* is not answerable when the task is small enough that any model succeeds.

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
{ "codebase": { "path": "../../fixtures/legacy-service" } }
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

## What the environment contains

Each dispatch gets its own private environment holding:

- the codebase, checked out at the resolved commit, with its history intact
- no remotes — nothing in the environment can reach or push to the source it came from
- hooks disabled, and a fixed committer identity for the runner's own commit
- the branch the codebase itself was on: the branch a `ref` names, or the repository's default
  branch when the ref is a tag or a SHA
- `refs/eval-magic/baseline`, marking the state the agent started from

An eval that declares no `codebase` still gets a Git repository, initialized on `work`, exactly as
it always has.

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

What counts is what Git counts, under the same rules the baseline commit was built under:

- The codebase's own `.gitignore` holds, so a run that compiles does not report its build output as
  thousands of touched files.
- Fixtures and staged skills count even when the codebase ignores their paths — they are committed
  into the baseline regardless, so a change to one is always visible.
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

A fixture overwrites a codebase file of the same path.

The baseline the runner commits respects the codebase's `.gitignore`, so ignored build output stays
out of it. Fixtures and staged skills are committed regardless of what the codebase ignores — which
is also what keeps them inside every later measurement.

## A `path` source is not reproducible elsewhere

Someone reading your published results cannot resolve `../../fixtures/legacy-service`. Their machine
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
