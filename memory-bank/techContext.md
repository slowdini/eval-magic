# Tech Context

## Stack

- **Language:** Rust, edition 2024, stable channel (pinned via `rust-toolchain.toml` with
  rustfmt + clippy components). `rustfmt.toml` sets `max_width = 100`.
- **CLI framework:** clap 4 (`derive`).
- **Runtime prerequisites:** none beyond the binary itself; Git is the only external runtime
  dependency.

## Dependency posture

No-network by design — see the rationale comments in `Cargo.toml`:

- `jsonschema` with `default-features = off` — schemas are embedded via `include_str!` and use
  only internal `#/definitions` refs, so no remote `$ref` resolver (no reqwest/tokio).
- `chrono` with `default-features = off` (only `alloc`) — parses RFC3339 timestamps, never reads
  the wall clock.
- `regex` with `default-features = off` + `std`/`perf` — shell-command patterns are ASCII by
  intent.
- `toml` with `parse`/`serde`/`display` — descriptors are serialized back to authorable TOML for
  `harness show`.
- Others: `anyhow`, `thiserror`, `serde`/`serde_json` (`preserve_order`), `similar` (diffing),
  `tempfile`, `walkdir`.
- Dev: `assert_cmd`, `predicates`, and `cargo-husky` (installs git hooks on first `cargo test`).

## Commands

```sh
cargo build                                        # debug build
cargo build --release                              # target/release/eval-magic
cargo run -- --help                                # explore the CLI tree
cargo test                                         # unit + integration + doc tests
cargo fmt --check                                  # formatting check (CI runs --all)
cargo clippy --all-targets -- -D warnings          # CI lint gate
```

CI (`.github/workflows/ci.yml`) runs fmt check, clippy (`-D warnings`), and the test suite on
PRs to `dev` and `main`. Git hooks via cargo-husky: pre-commit runs `fmt --check` + clippy,
pre-push runs the test suite.

## Release & distribution

- `dist-workspace.toml` drives cargo-dist; GitHub Releases carry prebuilt binaries for macOS
  (Apple Silicon + Intel), Linux (x64 + ARM64), Windows (x64), plus installer scripts; also on
  crates.io (`cargo install eval-magic`).
- `Cargo.toml` `exclude` keeps repo/agent config (`.claude/`, `.github/`, `AGENTS.md`, …) out of
  the crate tarball.

## Conventions

`AGENTS.md` is authoritative for coding style, naming (snake_case Rust, kebab-case CLI flags and
eval IDs), module placement, testing placement, and commit style (Conventional Commits, one
concern per commit).
