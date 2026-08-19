//! End-to-end integration tests for the `run` orchestrator and `teardown`,
//! driving the built `eval-magic` binary against an isolated CWD.
//!
//! clap owns dispatch, so a flagged invocation names the `run` subcommand
//! explicitly; a bare `eval-magic` with no args still defaults to `run`.
//!
//! Split into submodules (file-length guideline); shared fixtures and helpers
//! live in [`helpers`].

mod helpers;

mod agent_env;
mod byoh;
mod claude_cli;
mod cline;
mod cline_permission_denials;
mod codebase;
mod codex;
mod codex_guard;
mod codex_permission_denials;
mod command_check;
mod conversation;
mod diff_scope;
mod env_layout;
mod git_isolation;
mod grouping;
mod lifecycle;
mod opencode;
mod opencode_permission_denials;
mod runbook;
mod shadow_runtime_id;
mod skill_source;
mod staging;
mod statistical_floor;
