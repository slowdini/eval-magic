# Repository Guidelines

## Project Structure & Module Organization

`eval-magic` is a Rust CLI crate. The binary entry point is `src/main.rs`; reusable logic lives in
`src/lib.rs` and submodules such as `cli/`, `pipeline/`, `sandbox/`, `validation/`, and
`workspace/`. JSON schemas are tracked in `schema/`, harness profiles in `profiles/`, and design or
parity notes in `docs/`. Integration tests are split by surface area under `tests/cli/` and
`tests/run/`; unit tests usually live beside the module they exercise.

## Build, Test, and Development Commands

- `cargo build` builds the debug binary.
- `cargo build --release` builds the optimized `target/release/eval-magic` binary.
- `cargo run -- --help` checks the CLI tree locally.
- `cargo test` runs unit, integration, and doc tests.
- `cargo fmt --check` verifies formatting without rewriting files.
- `cargo clippy --all-targets -- -D warnings` catches common Rust issues before review.

## Documentation is a first-class citizen

CLI `--help` docs are the primary way that usage is discovered. Any new feature that has
user-facing elements must be thoroughly described in the shipped documentation.

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

## Commit & Pull Request Guidelines

Use concise Conventional Commit-style subjects such as `feat(codex): ...`, `fix(ci): ...`, and
`chore(docs): ...`. Keep commits scoped to one concern. Pull requests should explain the user-facing
change, list verification commands, link relevant issues, and call out schema, CLI, or documentation updates.
For output or workflow changes, include a short before/after example.
