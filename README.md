# eval-magic

A one-stop CLI for running skill evals — it measures whether an agent skill
actually shifts behavior. `eval-magic` is the Rust rewrite of
[`@slowdini/eval-runner`](https://github.com/slowdini/eval-runner), distributed
as a dependency-less prebuilt binary (no npm required) under the command name
`skill-eval`.

## Why Rust

- **Performance** — the runner is invoked many times during an eval run; the
  mechanical parts stay fast and lean.
- **Portability** — ships as a standalone binary for users who don't want to go
  through npm.

## Status

Early rewrite. The project skeleton mirrors eval-runner's module structure
(`core`, `validation`, `adapters`, `sandbox`, `pipeline`, `workspace`, `cli`),
and the CLI command surface is in place; per-command behavior is being ported
module-by-module following a test-first process. See
[`rewrite-roadmap.md`](./rewrite-roadmap.md) for the phased plan and current
progress.

## Development

```sh
cargo build              # debug build
cargo run -- --help      # explore the command surface
cargo test               # run tests (also installs git hooks via cargo-husky)
cargo fmt --all          # format
cargo clippy --all-targets --all-features -- -D warnings   # lint
```

Git hooks are installed automatically on first `cargo test`: pre-commit runs
`fmt --check` + `clippy`, pre-push runs the test suite.

## License

MIT
