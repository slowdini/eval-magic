//! End-to-end integration tests for the `run` orchestrator and `teardown`,
//! driving the built `skill-eval` binary against an isolated CWD.
//!
//! clap owns dispatch, so a flagged invocation names the `run` subcommand
//! explicitly; a bare `skill-eval` with no args still defaults to `run`.
//!
//! Split into submodules (file-length guideline); shared fixtures and helpers
//! live in [`helpers`].

mod helpers;

mod codex;
mod lifecycle;
mod staging;
