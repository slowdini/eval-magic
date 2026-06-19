# Eval-Magic Isolated Run

You are an agent (or a human) setting up or following a Claude Code **interactive** eval run. This
file fixes how such a run executes *inside an isolated environment* so the agent-under-test cannot
read the surrounding repo, sibling/installed plugins, or the *other* condition's skill. Read it
end-to-end before staging or dispatching.

This file covers the **Claude Code interactive** path (the `InSession` dispatch mechanism) — the
first milestone of the isolated-run epic (#74). The design is written to extend to the `Cli`
mechanism (Codex hybrid, headless) without changing the env layout; those land later (#82, #83).

**Evergreen:** when the env layout or the `switch-condition` contract changes, update this file in the
same PR. The decision recorded in §3 is the resolved output of design spike #77 — change it only with
a follow-up spike.

## 1 — Why isolation, and what it isolates

Today staging targets the current working directory: `RunContext.stage_root = cwd`
(`src/core/context.rs:204`). The agent-under-test therefore runs *inside the source tree* and can
read everything around it — the rest of the repo, installed plugins, and the staged copy of the other
condition's skill. These leaks are **confounds**: a result may be attributable to context that leaked
in, and you only find out (if at all) after the run.

The goal is a clean, per-iteration directory — `skills-workspace/<skill>/iteration-N/env/` — that
becomes the agent's cwd. Read isolation then comes *for free* from the same cwd boundary every harness
already enforces. Two distinct threats have to be addressed:

| Threat | What blocks it |
|--------|----------------|
| Reading the surrounding repo / installed plugins | Clean `env/` as cwd — nothing unrelated is inside it |
| The control arm reading the *other* condition's staged skill | Per-condition staging + the `switch-condition` barrier (§4) — the off-condition's skill is physically absent during a batch |
| A subagent writing outside the sandbox | `--guard` (pre-tool deny) while dispatches run; `detect-stray-writes` post-pass as the portable fallback |

**Honest caveat:** `detect-stray-writes` is **not** the backstop for the second threat inside `env/`.
Its live-source-read detection (`src/pipeline/detect_stray_writes.rs:178-222`) flags reads of the
*live* skill-under-test directory — but in `env/` only the *staged copy* is present, never the live
dir, so a control arm reading the staged copy is invisible to it. Isolation here rests on the skill
being physically gone/swapped (§4), not on post-hoc detection.

## 2 — Directory layout: `env/` vs `iteration-N/` above it

eval-magic meta lives **above** the env; only the clean `env/` is the agent's cwd.

| Path | Owner | Contents | Agent-visible? |
|------|-------|----------|----------------|
| `iteration-N/` | eval-magic | `conditions.json`, `dispatch.json`, `benchmark.json`, `stray-writes.json`, `skill-snapshot.md`, and the per-run `eval-<id>/<condition>[/run-k]/` trees (`run.json`, `timing.json`, `grading.json`) | **No** |
| `iteration-N/env/` | the run | The agent's cwd. Clean; fixtures copied in so it reads like a real repo | Yes (it *is* the cwd) |
| `iteration-N/env/.claude/skills/` | the run | The staged skill for the **current condition batch only** (becomes the guard `skills_dir` once `stage_root = env/`, `src/sandbox/install.rs:136`) | Yes |

The agent never needs to look above `env/`. eval-magic does — it reads and writes the meta tree as a
trusted binary (§5).

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

`switch-condition` mutates `env/.claude/skills/` between condition batches, reusing the existing
per-condition staging and cleanup primitives (`stage_skill_for_harness`, `cleanup_staged_skills` in
`src/cli/run/staging/mod.rs`; the per-condition slugs `cond_a_slug`/`cond_b_slug` in
`src/cli/run/orchestrate/stage.rs`).

- **New-skill:** stage the `with_skill` slug → dispatch **and join** that batch → `switch-condition`
  **removes** `env/.claude/skills/<with_skill-slug>/` → dispatch the `without_skill` batch. The files
  are gone, so the control arm cannot read them.
- **Revision:** `switch-condition` performs an **in-place content swap** at a path that already
  existed at session start, so Claude's live change detection propagates it
  (`src/cli/run/util.rs:65-74`). No watched directory is created mid-session.

> **Hard contract — `switch-condition` is a barrier.** The orchestrator MUST join *all* Task
> subagents of the current batch before switching. A subagent still in flight when the skill is
> removed could observe a half-removed directory or a failed discovery, tainting the run. Conditions
> run sequentially, batch by batch; never interleave them.

**Watcher caveat.** `env/.claude/skills/` MUST exist *before* the isolated session starts. Claude
Code only watches skill directories that existed at session start; a directory created mid-session
isn't watched (`src/cli/run/util.rs:56-94`, `src/cli/run/orchestrate/stage.rs:32`). Populating the env
before session B begins is exactly what makes the fresh session *structural* and removes the
watcher hazard — so `switch-condition` only ever mutates *contents* (remove a slug, or swap a file's
content), never creates the watched dir fresh.

**Guard note.** The guard marker (`.slow-powers-eval-guard.json`, `src/sandbox/install.rs`) is a
**sibling** of the `<slug>/` subtree inside `skills_dir`, not nested within it. `switch-condition`
removes only `<slug>/` and must leave the marker intact; decide explicitly whether to re-arm the guard
(refresh its TTL) for the second batch.

## 5 — The loop in one session: dispatch → ingest → finalize

Because there is exactly one isolated session, one `CLAUDE_CODE_SESSION_ID` resolves **both**
conditions' subagent transcripts in-session (`resolve_subagents_dir`, `src/cli/mod.rs:179-216`). No
`--session-id` / `--subagents-dir` is needed — that cross-session "dance" is precisely what staying
in one session avoids (`src/cli/run/dispatch.rs:443`; #79).

`benchmark.json` aggregates across both conditions and is written into `iteration-N/`, *above* `env/`.
This is allowed because eval-magic is a **trusted binary** writing within the guard's `allowedRoots`
— `[workspace_root, skills_dir, temp]` (`src/sandbox/install.rs:71-77`) — which includes the
workspace iteration tree. The guard bounds the *agent's* writes to its cwd; it does not bind
eval-magic, which writes the meta tree by design.

## 6 — Validation checklist (the spike's "one real Claude-interactive run")

These are the empirical assumptions the design rests on. They are to be confirmed by a real
Claude-interactive run once the env builder (#78) and full-loop handoff (#79) exist; the design note
records them so those tickets execute against a fixed contract.

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
