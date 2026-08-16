# Repository Guidelines

## Project Structure & Module Organization

`eval-magic` is a Rust CLI crate. The binary entry point is `src/main.rs`; reusable logic lives in
`src/lib.rs` and submodules such as `cli/`, `pipeline/`, `sandbox/`, `validation/`, and
`workspace/`. JSON schemas are tracked in `schema/`, harness descriptors (plus embedded harness
assets such as the OpenCode write-guard plugin template) in `harnesses/`, shared
prompt profiles in `profiles/`, and docs in `docs/` — user-facing Markdown under `docs/guides/`
ships embedded in the binary as `eval-magic docs <topic>`; the other files are internal development
docs. `docs/developer_overview.md` maps the repository and holds the placement policy. Integration
tests are split by surface area under `tests/cli/` and `tests/run/`; unit tests usually live beside
the module they exercise.

## Build, Test, and Development Commands

- `cargo build` builds the debug binary.
- `cargo build --release` builds the optimized `target/release/eval-magic` binary.
- `cargo run -- --help` checks the CLI tree locally.
- `cargo test` runs unit, integration, and doc tests.
- `cargo fmt --check` verifies formatting without rewriting files.
- `cargo clippy --all-targets -- -D warnings` catches common Rust issues before review.

## Documentation is a first-class citizen

CLI `--help` docs are the primary way that usage is discovered. Any new feature that has
user-facing elements must be thoroughly described in the shipped documentation. Shipped means the
`--help` tree plus the reference topics embedded in the binary and printable via
`eval-magic docs <topic>` (topics are discovered from `docs/guides/*.md`; shipped output references
them as `eval-magic docs <topic>`, never repo-relative paths). What belongs where is governed by
`docs/developer_overview.md`.

## Coding Style & Naming Conventions

This repo uses Rust 2024 with `rustfmt` configured for `max_width = 100`. Keep modules, functions,
variables, and test names in `snake_case`; CLI flags and eval IDs should be kebab-case
(`--skill-dir`, `claim-without-running`). Prefer small modules with focused responsibilities, and keep
the binary thin: new behavior should generally live in the library crate so it stays testable.

## Testing Guidelines

Add unit tests near the implementation for parsing, validation, and pure logic. Add integration tests
under `tests/cli/` or `tests/run/` when behavior crosses the command-line boundary or writes
workspace artifacts. Use descriptive test names that state the behavior, for example
`snapshot_ref_reads_committed_content`. Run `cargo test` before handing off changes; include
formatting and clippy checks when touching Rust code.

**Where unit tests live.** An inline `#[cfg(test)] mod tests` at the bottom of the file it exercises
is the default — most modules use it. Extract only when that module outgrows its file: either to a
`<module>/tests/` directory of themed submodules (as `pipeline/record_runs/` and `cli/run/staging/`
do) or to a single `<topic>_tests.rs` sibling (as `adapters/guard/guard_denial_tests.rs` does).
Extraction is a size decision, not a style preference; don't split a small inline module.

**Spawning a child process from a test.** Use the hidden `__fixture` subcommand, never `sh`, `true`,
`printf`, or a `#!/bin/sh` stub. It exits with a chosen code, emits chosen bytes, writes a chosen
file, or checks a file or variable — see `FixtureArgs` in `src/cli/args.rs`. One invocation parses
the same under `sh -c` and `cmd /C`, which is what keeps `command_check` tests off per-OS command
strings. Build the command with the `fixture` helper (`tests/run/helpers.rs` for integration tests,
the one in `src/pipeline/grade/command_check/tests.rs` for unit tests). Because the fixture is the
binary, `cargo test --lib` alone does not build it — run `cargo test`, or `cargo build` first.

**Tests are gated on capabilities, not on the OS.** `#[cfg(unix)]` on a test hides it from
compilation and clippy on the other host and hides the coverage gap. Instead, probe for what the
test actually needs and call `report_skip` (`src/core/runtime.rs`), which prints the reason and
returns `true`. Setting `EVAL_MAGIC_REQUIRE_POSIX_TOOLS=1` turns every skip into a failure; CI sets
it on both runners, so neither can quietly stop covering something. Three capabilities are gated
today: the recipe tools beyond the shell itself (`require_posix_toolchain` — in practice `jq`),
symlink creation, which Windows allows only under Developer Mode, and creating a path past
Windows' 259-character limit (`deep_task_root`, `src/cli/run/orchestrate/git.rs`). The Windows
runner is provisioned for those rather than exempted from them, so a skip there is a red build. The
shell is not one of them; it is a hard requirement, per the section below.
`require_posix_toolchain` is not test-only either — the `run` preflight uses it to warn about the
same gap. Where a genuine per-OS difference is the behavior under test — signals, path separators —
branch on `cfg!(windows)` at runtime so both arms still compile everywhere.

**A POSIX shell is required, for use and for development.** Harness `exec_template`s are POSIX
command lines, so the dispatch and probe paths spawn `sh` via `posix_shell()`
(`src/core/runtime.rs`) rather than a hardcoded `/bin/sh`: it searches `PATH`, then a Git for
Windows install. Set `EVAL_MAGIC_SH` to override it. `cargo test` inherits the requirement — the
scripted-turn tests spawn a `#!/bin/sh` harness stub through the resolved shell and do not skip —
so a host without `sh` fails the suite instead of quietly covering less. `jq` is required alongside
it for the parallel-dispatch and judge recipes; Git for Windows supplies the shell, `xargs`, `tr`,
and `wc`, but not `jq`. `POSIX_TOOLING_REQUIREMENT` (`src/core/runtime.rs`) is the one wording the
Markdown-carrying surfaces reuse: the shell-discovery errors, the `run` preflight warnings,
`RUNBOOK.md`, and `dispatch-manifest.md`. State the requirement from there rather than rephrasing
it. `--help` is the one deliberate restatement (`AFTER_HELP` in `src/cli/help.rs`), hard-wrapped and
backtick-free because clap renders into a terminal; keep the two in step by hand.

Which platforms that requirement is honored on — and why preparing on Windows but dispatching from
WSL is a correctness boundary rather than a preference — is stated once under "Platform support" in
`docs/developer_overview.md`.

**Where user-facing warnings come from.** Library modules (`pipeline`, `workspace`, `sandbox`,
`adapters`) never print. They return warning strings on their result struct — `#[serde(skip)]` when
that struct is also a serialized artifact — and the `cli` handler prints them with the `⚠ ` prefix.
This keeps warnings testable without capturing stderr and keeps one place deciding how they read.

## Commit & Pull Request Guidelines

Use concise Conventional Commit-style subjects such as `feat(codex): ...`, `fix(ci): ...`, and
`chore(docs): ...`. Keep commits scoped to one concern. Pull requests should explain the user-facing
change, list verification commands, link relevant issues, and call out schema, CLI, or documentation updates.
For output or workflow changes, include a short before/after example.
