# Developer overview

> **Audience:** contributors changing eval-magic itself. For installed-tool usage, start with
> `eval-magic --help`; for harness authoring and dispatch isolation, run `eval-magic docs`.

eval-magic is a Rust CLI that builds reproducible skill-evaluation campaigns, hands dispatches to
an agent harness, ingests what happened, grades the results, and preserves the evidence needed to
compare conditions. This page is the repository map and documentation-placement policy for new
contributors. It deliberately points to authoritative code, schemas, generated artifacts, and
focused internal notes instead of duplicating their details.

## How an evaluation moves through the system

1. `eval-magic init` scaffolds an eval workspace next to a skill. Eval definitions describe the
   task, fixtures, assertions, conditions, run count, and optional scripted follow-up turns.
2. `eval-magic run` validates the configuration, creates isolated task roots, stages the requested
   skill condition, snapshots the starting state, and writes `RUNBOOK.md`, `dispatch.json`, and
   related campaign artifacts. The generated runbook—not a checked-in recipe—is the authority for
   dispatching that particular campaign.
3. An operator or automation dispatches each task with the selected harness. One-shot tasks invoke
   the harness once; scripted conversations use `eval-magic dispatch-task` to preserve one native
   harness session across turns.
4. `eval-magic ingest` reads the harness outputs, transcript evidence, guard denials, and final
   task state. Runner-owned deterministic checks and diff-scope evidence are collected here.
5. `eval-magic grade` evaluates runner-owned assertions and emits tasks for assertions that require
   an LLM. The generated recipes dispatch those judge tasks through the selected harness.
6. `eval-magic finalize` checks that required work is complete and writes the final per-run and
   benchmark artifacts. `eval-magic aggregate` combines campaigns when a larger comparison is
   needed.
7. `eval-magic teardown` removes staged skills and temporary guard configuration. Campaign
   artifacts remain available for audit and comparison.

Use each command's `--help` before changing a phase. It documents the current inputs, outputs,
preconditions, handoffs, and recovery commands.

## Repository map

- `src/main.rs` is the thin binary entry point; `src/lib.rs` exposes reusable crate logic.
- `src/cli/` owns argument parsing, user-facing help, command handlers, and presentation. Library
  modules return data and warnings; the CLI decides what to print.
- `src/pipeline/` owns campaign phases and artifact assembly.
- `src/adapters/` loads harness descriptors and implements the shared adapter boundary plus the
  few named capabilities that require harness-specific code.
- `src/sandbox/`, `src/workspace/`, and `src/validation/` own task isolation, filesystem/workspace
  mechanics, and configuration checks.
- `src/source/` resolves a declared source — a git URL and ref, or a local directory — to a commit,
  and materializes it as a tree. It knows nothing about what is being sourced, so both the codebase
  a task environment is built from and the skills under test resolve through it.
- `schema/` contains the JSON schemas for user input and generated artifacts.
- `harnesses/` contains built-in descriptors, descriptor scaffolding, and embedded harness assets.
- `profiles/` contains shared prompt profiles.
- `tests/cli/` covers CLI and packaging contracts; `tests/run/` covers campaign behavior across
  the run boundary. Focused unit tests normally live beside the implementation.
- `docs/guides/` contains Markdown guides embedded in the binary. Other files under `docs/` are
  internal contributor notes.

## Sources of truth

When prose and a machine-readable or generated surface disagree, fix the prose against the
following authorities:

- `eval-magic <command> --help` for the installed CLI contract.
- The relevant file under `schema/` for accepted fields and serialized artifact shapes.
- Built-in and layered descriptor data for harness behavior. Use `eval-magic harness list` to see
  registered harnesses and `eval-magic harness show <label>` to inspect the resolved descriptor.
- A campaign's generated `RUNBOOK.md`, `dispatch.json`, and `dispatch-manifest.md` for the exact
  commands and handoffs of that run.
- Real harness `--help`, vendor documentation, and observed output for descriptor values. Never
  infer one harness's flags or event shapes from another harness.
- Tests and golden artifacts for behavior that crosses a module or CLI boundary.

## Platform support

| Tier | Platform | Verified by |
| --- | --- | --- |
| Supported | Linux, macOS | the `ubuntu-latest` CI job |
| Deprecated | Windows, through Git Bash (Git for Windows) | the `windows-latest` CI job |
| Unsupported | preparing a workspace on Windows and dispatching it from WSL | — |

Windows support is deprecated in favor of WSL, and its removal is gated on #256, which replaces
the generated POSIX recipes with a runner-driven `eval-magic dispatch`. Until that lands, the
Windows runner stays green and Windows-native behavior is held to the same bar as any other
platform: a Windows failure is a real failure, not an accepted gap. Do not add new Windows-native
accommodation in the meantime.

The unsupported row is a correctness boundary rather than a preference. A generated recipe carries
the absolute paths of the host that prepared the workspace. Git Bash shares the Windows filesystem,
so those paths resolve; WSL resolves its own namespace, where a `C:\…` path names nothing. Nothing
in the tree translates between the two, so the split fails quietly instead of loudly.
`POSIX_TOOLING_REQUIREMENT` (`src/core/runtime.rs`) is the single wording every user-facing surface
reuses to state this; `src/cli/help.rs` restates it for clap by hand.

## Make and verify a change

Trace the user-visible behavior from the CLI handler into library-owned logic and artifacts before
editing. Add a focused failing test at the narrowest useful boundary, implement the change, then
run the focused test again. Cross-harness changes belong at shared descriptor, runner, or adapter
boundaries unless the evidence requires a named harness capability.

Development carries the host requirement the tool itself declares: a POSIX shell with `jq`. The
scripted-turn tests spawn `#!/bin/sh` harness stubs through the resolved shell and do not skip, so
the suite cannot pass without one. Tests needing `jq`, symlink creation, or a path past Windows'
259-character limit report a skip instead; `EVAL_MAGIC_REQUIRE_POSIX_TOOLS=1` turns those skips into
failures, as CI sets it to do on both its Ubuntu and its Windows runner.

Before handing work off, run:

```text
cargo fmt --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
git diff --check
```

Add schema checks, help snapshots, guide-contract tests, or golden-artifact verification when the
changed surface calls for them.

## Documentation policy

Documentation is a shipped feature. Put each fact on the surface where its audience will look:

1. **CLI help** (`src/cli/args.rs` and `src/cli/help.rs`) is the primary discovery surface. Every
   command and flag needs enough context to use it correctly and find the next handoff.
2. **Shipped guides** are Markdown files directly under `docs/guides/`. `build.rs` discovers every
   `.md` file at compile time: the filename stem is the `eval-magic docs <topic>` name, the first H1
   is its listing title, and the complete file is embedded verbatim. Guide names must be ASCII
   kebab-case. Shipped output refers to guides with `eval-magic docs <topic>`, not repository paths.
3. **Generated run documentation** such as `RUNBOOK.md` is the authority for commands whose exact
   form depends on a campaign, harness, or model. Do not duplicate those recipes in static prose.
4. **Internal development docs** explain architecture, evidence, and maintenance contracts for
   contributors. They may link to repository paths, but installed-tool users must not depend on
   them.

Keep the README as a short landing page: what the tool is, how to install it, one successful first
run, and where to continue. Distill operational detail into CLI help or a shipped guide; retain
implementation evidence in an internal note.

## Internal guide index

- [Harness progressive enhancements](progressive-enhancements.md) defines the baseline adapter
  contract, optional enhancements, fallbacks, and contribution boundaries.
- [Claude Code notes](claude-notes.md), [Cline notes](cline-notes.md), [Codex notes](codex-notes.md), and
  [OpenCode notes](opencode-notes.md) record harness-specific evidence and maintenance details.
- [Shipped harness-authoring guide](guides/byoh.md) is the repository source for
  `eval-magic docs byoh`.
- [Shipped isolation guide](guides/isolation.md) is the repository source for
  `eval-magic docs isolation`.
- [Shipped codebase guide](guides/codebase.md) is the repository source for
  `eval-magic docs codebase`.
