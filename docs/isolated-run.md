# Eval-Magic Isolated Run

You are an agent (or a human) setting up or following a Claude Code **interactive** eval run. This
file fixes how such a run executes *inside an isolated environment* so the agent-under-test cannot
read the surrounding repo, sibling/installed plugins, or the *other* condition's skill. Read it
end-to-end before staging or dispatching.

This file covers the **Claude Code interactive** path (the `InSession` dispatch mechanism) — the
first milestone of the isolated-run epic (#74). The same env layout also serves the `Cli` mechanism
unchanged: Claude Code and Codex hybrid/headless runs share this env and the human-followed runbook,
differing only in the dispatch recipe and who drives the loop.

**Evergreen:** when the env layout or the `switch-condition` contract changes, update this file in the
same PR. The decision recorded in §3 is the resolved output of design spike #77 — change it only with
a follow-up spike.

## 1 — Why isolation, and what it isolates

Staging historically targeted the current working directory (`RunContext.stage_root` defaults to
`cwd`, `src/core/context.rs:204`), so the agent-under-test ran *inside the source tree* and could
read everything around it — the rest of the repo, installed plugins, and the staged copy of the other
condition's skill. These leaks are **confounds**: a result may be attributable to context that leaked
in, and you only find out (if at all) after the run.

The env builder (#78) redirects staging into a clean, per-iteration directory —
`skills-workspace/<skill>/iteration-N/env/` — that becomes the agent's cwd: `command_run` rebinds
`stage_root` to `iteration-N/env/` once the iteration is resolved
(`src/cli/run/orchestrate/mod.rs`), and the existing root-parameterized staging path follows. Read
isolation then comes *for free* from the same cwd boundary every harness already enforces. Two
distinct threats have to be addressed:

| Threat | What blocks it |
|--------|----------------|
| Reading the surrounding repo / installed plugins | Clean `env/` as cwd — nothing unrelated is inside it |
| The control arm reading the *other* condition's staged skill | Per-condition staging + the `switch-condition` barrier (§4) — the off-condition's skill is physically absent during a batch |
| A subagent writing outside the env | The cwd boundary bounds the agent's direct file tools to `env/`; `--guard` (pre-tool deny, scoped to `env/`) additionally blocks Bash-subprocess escapes the cwd boundary misses (installs, `git worktree`, redirects); `detect-stray-writes` post-pass as the portable fallback (#81) |

**Honest caveat:** `detect-stray-writes` is **not** the backstop for the second threat inside `env/`.
Its live-source-read detection (`src/pipeline/detect_stray_writes.rs:178-222`) flags reads of the
*live* skill-under-test directory — but in `env/` only the *staged copy* is present, never the live
dir, so a control arm reading the staged copy is invisible to it. Isolation here rests on the skill
being physically gone/swapped (§4), not on post-hoc detection.

## 2 — Directory layout: `env/` vs `iteration-N/` above it

eval-magic meta lives **above** the env; only the clean `env/` is the agent's cwd.

| Path | Owner | Contents | Agent-visible? |
|------|-------|----------|----------------|
| `iteration-N/` | eval-magic | `conditions.json`, `dispatch.json`, `dispatch-manifest.md`, `RUNBOOK.md` (see below), `benchmark.json`, `stray-writes.json`, `skill-snapshot.md`, and the per-run `eval-<id>/<condition>[/run-k]/` trees (`run.json`, `timing.json`, `grading.json`) | **No** |
| `iteration-N/env/` | the run | The agent's cwd. Clean; fixtures copied in so it reads like a real repo | Yes (it *is* the cwd) |
| `iteration-N/env/.claude/skills/` | the run | The staged skill for the **current condition batch only** (becomes the guard `skills_dir` once `stage_root = env/`, `src/sandbox/install.rs:136`) | Yes |
| `iteration-N/env/.eval-magic/outputs/<eval>/<cond>[/run-k]/` | the run | Where each dispatched subagent writes its files + `final-message.md`. Per-`(eval, condition, run)` so concurrent same-batch subagents can't collide; `record-runs` reads `final-message.md` from here. Inside the env so the agent never writes above its cwd | Yes (inside the cwd) |

The agent never needs to look above `env/`. eval-magic does — it reads and writes the meta tree as a
trusted binary (§5).

### Runbook (#69)

`run` generates `RUNBOOK.md` — a followable handoff the isolated session reads end-to-end ("Read and
follow RUNBOOK.md"). The per-mode prose skeletons are checked in under `profiles/` and carry
`{{TOKEN}}` placeholders the run fills with run-specific values
(`src/cli/run/runbook.rs`); the template is selected by the harness's
`DispatchMechanism` (`runbook_template`, `src/adapters/harness.rs`):

- **`InSession` (Claude Code) → interactive, agent-followed** (`profiles/claude-code/runbook.md`): an
  agent in a fresh session dispatches the subagents and runs the whole `ingest → finalize → teardown`
  loop itself.
- **`Cli` (Claude Code hybrid/headless, Codex, OpenCode) → human-followed** (`profiles/shared/runbook-headless.md`): a
  human (headless) or an orchestrating agent (hybrid) pastes the harness CLI dispatch recipe (from the
  adapter's `cli_*` generators) and the pipeline commands.

`RUNBOOK.md` is the single source of the in-session dispatch loop (built from the shared
`insession_dispatch_batch` / `insession_switch_command` / `insession_ingest_command` fragments in
`src/cli/run/util.rs`). The post-`run` summary no longer reprints that loop — it just hands off:
"cd into `env/`, start a fresh session, *Read and follow RUNBOOK.md*" (`insession_isolated_handoff`,
`src/cli/run/util.rs`).

**Status.** The env builder (#78) and the full-loop handoff (#79) have landed. Staging, copied
fixtures, and `RUNBOOK.md` are written into `iteration-N/env/` — the isolated session's cwd — while
eval-magic meta (and the per-run `eval-<id>/` `run.json`/`timing.json` trees) stay above it in
`iteration-N/`. The isolated session now drives the **whole loop** itself: it dispatches each
condition as a batch, runs `eval-magic switch-condition` between batches (the §4 barrier), then
`ingest → finalize`, writing `benchmark.json` into `iteration-N/`. Per-task dispatch outputs live
inside the env at `env/.eval-magic/outputs/` (§2), and every printed/runbook command carries an
absolute `--workspace-dir` (`command_target_args`) so it resolves the iteration tree from
`cwd = env/`. The generated `RUNBOOK.md` is a workspace artifact and is **not** version controlled
(`skills-workspace/` is gitignored); only the `profiles/` templates are checked in.

## 3 — The condition / dispatch model under Claude's subagent model

In Claude Code, Task subagents inherit the orchestrator session's cwd. A single `env/` therefore
**cannot** physically hide the staged skill from a co-resident control arm: whatever sits in
`env/.claude/skills/` is readable by every subagent dispatched from that session.

The conditions per mode (`condition_names_for`, `src/cli/run/util.rs:17`):

- **New-skill** — `with_skill` (skill staged) vs `without_skill` (no skill).
- **Revision** — `old_skill` vs `new_skill`; both have a skill, only the *content* differs.

**Resolved decision (spike #77): one isolated session, sequential per-condition batches, with a
`switch-condition` barrier between them.** The off-condition's skill is made physically absent (or
swapped) on disk before that condition's batch runs, so there is nothing to leak.

| Option | Verdict |
|--------|---------|
| **(a) One session + `switch-condition`** | **Chosen.** One session runs the whole loop; the staged skill is removed/swapped between batches. Preserves in-session transcript resolution (§5), fits the singular `env/` layout, and delivers real read isolation. |
| (b) Separate session per condition (`env-with_skill/`, `env-without_skill/`) | Rejected. Strongest *physical* isolation, but each session has its own `CLAUDE_CODE_SESSION_ID`, so the loop can auto-resolve only one condition's transcripts in-session — the other forces the cross-session `--subagents-dir` dance #79 exists to kill. Also breaks the singular `env/` layout. |
| (c) One env, weaker isolation | Rejected. Both staged skills coexist (today's behavior, `src/cli/run/orchestrate/stage.rs:129-130`); relies on the dispatch prompt + `detect-stray-writes`, which is blind to staged-copy reads inside `env/` (§1). |

## 4 — The `switch-condition` mechanism

`switch-condition` mutates `env/.claude/skills/` between condition batches. The handler
(`run_switch_condition`, `src/cli/commands/pipeline.rs`) reads the off-condition's recorded
`staged_skill_slug` from `conditions.json` and removes exactly that slug subtree with
`fs::remove_dir_all`. It deliberately does **not** call `cleanup_staged_skills`
(`src/cli/run/staging/mod.rs`) — that prefix-scans and would remove *both* arms' slugs and prune the
dir; only the one off-condition slug must go. `--condition` names the arm to **keep** (the one about to
be dispatched); its counterpart is the off-condition.

- **New-skill:** stage the `with_skill` slug → dispatch **and join** that batch → `switch-condition`
  **removes** `env/.claude/skills/<with_skill-slug>/` → dispatch the `without_skill` batch. The files
  are gone, so the control arm cannot read them.
- **Revision:** both arms are staged at `run` time (the `old_skill` and `new_skill` slugs), so
  `switch-condition` is the **same primitive** — it removes the off-condition's slug
  (`<old_skill-slug>/`) before the `new_skill` batch, leaving only the kept arm's slug, which existed
  at session start and is therefore already watched. No content is rewritten, and no watched directory
  is created mid-session. (This supersedes the earlier "in-place content swap" sketch: with both arms
  staged up front, removing the off-condition slug is simpler and uniform across modes.)

> **Hard contract — `switch-condition` is a barrier.** The orchestrator MUST join *all* Task
> subagents of the current batch before switching. A subagent still in flight when the skill is
> removed could observe a half-removed directory or a failed discovery, tainting the run. Conditions
> run sequentially, batch by batch; never interleave them.

**Watcher caveat.** `env/.claude/skills/` MUST exist *before* the isolated session starts. Claude
Code only watches skill directories that existed at session start; a directory created mid-session
isn't watched. Populating the env before session B begins is exactly what makes the fresh session
*structural* and removes the watcher hazard — so `switch-condition` only ever mutates *contents*
(remove a slug, or swap a file's content), never creates the watched dir fresh. Because the env is
always built before the dispatching session starts, the hazard never bites in practice, so it needs
no in-session warning: the build-time discovery warnings and the session-juggling "Next:" loop were
retired in #80, leaving the clean cd-into-`env/` handoff.

**Guard note.** The guard marker (`.slow-powers-eval-guard.json`, `src/sandbox/install.rs`) is a
**sibling** of the `<slug>/` subtree inside `skills_dir`, not nested within it, so removing only
`<slug>/` leaves it — and an armed guard — intact. `switch-condition` does **not** re-arm or refresh
the guard's TTL: the 6h TTL comfortably covers a sequential two-batch run, so the barrier stays a pure
remove-the-slug operation. (`tests/run/switch_condition.rs` locks the marker's survival.)

## 5 — The loop in one session: dispatch → ingest → finalize

Because there is exactly one isolated session, one `CLAUDE_CODE_SESSION_ID` resolves **both**
conditions' subagent transcripts in-session (`resolve_subagents_dir`, `src/cli/mod.rs:179-216`). No
`--session-id` / `--subagents-dir` is needed — that cross-session "dance" is precisely what staying
in one session avoids (`src/cli/run/dispatch.rs:443`; #79).

`benchmark.json` aggregates across both conditions and is written into `iteration-N/`, *above* `env/`.
This is allowed because eval-magic writes the meta tree as a **subprocess** the agent launches via
Bash (`eval-magic ingest` / `finalize`), and the guard hook only inspects the agent's *own* tool
calls — the file writes of a subprocess it spawns are never intercepted. `eval-magic finalize` is a
non-mutating command the guard's Bash classifier passes, so the meta-tree writes proceed regardless
of the allowed roots. The guard's `allowedRoots` are therefore scoped tight to the env —
`[stage_root (env/), temp]` (`marker_allowed_roots`, `src/sandbox/install.rs`) — bounding the
*agent's* direct writes to its cwd and nothing above it (no sibling iteration, no meta tree). Scoping
to the env, not the parent `skills-workspace/`, keeps the guard boundary identical to the isolation
boundary (#81).

## 6 — Validation checklist (the spike's "one real Claude-interactive run")

These are the empirical assumptions the design rests on. Now that the env builder (#78) and full-loop
handoff (#79) have landed, they are confirmed by a real Claude-interactive run (the dogfood run that
gates #79); the design note records them so the contract stays fixed.

1. **Watcher retraction on delete (riskiest).** After `env/.claude/skills/<slug>/` is removed
   mid-session, a `without_skill` subagent (a) does not *discover* the skill in its available-skills
   block, and (b) cannot *read* the file by path.
2. **Content-swap propagation (Revision).** After an in-place content swap at a path present at
   session start, a subsequently-dispatched subagent sees the *new* content.
3. **Single-session both-condition loop.** One session runs `ingest` → `finalize` resolving *both*
   conditions' transcripts via `CLAUDE_CODE_SESSION_ID` with no `--subagents-dir`, and writes
   `benchmark.json` into `iteration-N/` without tripping the guard.
4. **Guard marker survives the switch.** `switch-condition`'s removal leaves the sibling guard marker
   intact.

## 7 — Alternatives considered / out of scope

- **(b) separate session per condition** — strongest physical isolation; rejected because it
  reintroduces the cross-session `--subagents-dir` dance (#79) and breaks the singular `env/` layout.
- **(c) one env, weaker isolation** — simplest; rejected because it fails the epic's read-isolation
  goal and `detect-stray-writes` is blind to staged-copy reads inside `env/` (§1).
- **Filesystem-level isolation (per-condition mount namespaces / overlay / chroot)** — would give the
  control arm an empty view of `env/.claude/skills/` without deleting files, sidestepping the
  watcher-retraction question entirely. It is the strongest option, but OS-specific and outside Claude
  Code's "subagents inherit cwd" model (no per-subagent mount namespace is exposed). Future work, not
  this milestone.

## 8 — Guardrails

- **`env/` is the agent's only cwd.** eval-magic meta stays above it in `iteration-N/`; the agent
  never reads or writes above `env/`.
- **`switch-condition` is a barrier.** Join every Task subagent of a batch before switching; never
  interleave conditions.
- **`env/.claude/skills/` must pre-exist the isolated session.** Populate the env before session B
  starts so the fresh session is structural, not a watcher workaround.
- **`detect-stray-writes` is not the isolation backstop inside `env/`.** Physical removal/swap of the
  off-condition skill is. Treat a clean stray-writes report as necessary, not sufficient, for
  per-condition read isolation.
- **Keep this file evergreen.** Update the env layout (§2) and the `switch-condition` contract (§4) here
  whenever they change, in the same PR.
