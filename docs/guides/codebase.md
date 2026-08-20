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
out of it. Fixtures and staged skills are committed regardless of what the codebase ignores.

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

The resolved commit appears in `conditions.json`, each `run.json`, `benchmark.json`, and the
`BASELINE.md` written by `promote-baseline` — alongside the skill the run measured, which
is recorded the same way:

```sh
jq '.codebases, .skill_source' conditions.json
```

See `eval-magic docs isolation` for what the skill side of that record means.
