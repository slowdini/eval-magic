//! Shared kernel used by nearly every other module.
//!
//! - [`types`]        — domain types (`Eval`, `RunRecord`, `Assertion`, `GradingResult`, …)
//! - [`context`]      — `RunContext` detection from parsed flags / environment
//! - [`capabilities`] — per-harness run-option capabilities
//! - [`git`]          — git spawned with the operator's configuration held off
//! - [`runtime`]      — runtime helpers (plain git spawning, POSIX shell discovery)
//!
//! The submodules are re-exported flat here so downstream code writes
//! `crate::core::Eval` rather than `crate::core::types::Eval`.

pub mod capabilities;
pub mod context;
pub mod fs;
pub mod git;
pub mod runtime;
pub mod types;

pub use capabilities::HarnessRunCapabilities;
pub use context::{ContextError, DetectInput, Harness, RunContext, detect_run_context};
pub use git::BASELINE_REF;
pub(crate) use git::IsolatedGit;
pub(crate) use runtime::{
    GIT_ROUTING_ENV_VARS, POSIX_RECIPE_TOOLS, POSIX_TOOLING_REQUIREMENT, clear_git_environment,
    posix_shell, require_posix_toolchain, validate_agent_environment_entry,
};
pub use runtime::{GitOutput, run_git};
pub use types::*;
