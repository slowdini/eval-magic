# Repository Guidelines

## Project Structure & Module Organization

`eval-magic` is a Rust CLI crate. The binary entry point is `src/main.rs`; reusable logic lives in
`src/lib.rs` and submodules such as `cli/`, `pipeline/`, `sandbox/`, `validation/`, and
`workspace/`. JSON schemas are tracked in `schema/`, harness descriptors (plus embedded harness
assets such as the OpenCode write-guard plugin template) in `harnesses/`, shared
prompt profiles in `profiles/`, and docs in `docs/` — user-facing `byoh.md` and `isolation.md` ship
embedded in the binary (the `eval-magic docs byoh` and `eval-magic docs isolation` topics), the rest
are internal development docs (the harness enhancement contract, per-harness notes);
`docs/README.md` holds the placement policy. Integration tests are split by surface area under
`tests/cli/` and `tests/run/`; unit tests usually live beside the module they exercise.

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
`eval-magic docs <topic>` (topics are registered in `src/cli/commands/docs.rs`; shipped output
references them as `eval-magic docs <topic>`, never repo-relative paths). What belongs where is
governed by `docs/README.md`.

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

**Where user-facing warnings come from.** Library modules (`pipeline`, `workspace`, `sandbox`,
`adapters`) never print. They return warning strings on their result struct — `#[serde(skip)]` when
that struct is also a serialized artifact — and the `cli` handler prints them with the `⚠ ` prefix.
This keeps warnings testable without capturing stderr and keeps one place deciding how they read.

## Commit & Pull Request Guidelines

Use concise Conventional Commit-style subjects such as `feat(codex): ...`, `fix(ci): ...`, and
`chore(docs): ...`. Keep commits scoped to one concern. Pull requests should explain the user-facing
change, list verification commands, link relevant issues, and call out schema, CLI, or documentation updates.
For output or workflow changes, include a short before/after example.
