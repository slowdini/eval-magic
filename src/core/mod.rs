//! Shared kernel used by nearly every other module.
//!
//! Mirrors `src/core/` in eval-runner:
//! - [`types`]   — domain types (`Eval`, `RunRecord`, `Assertion`, `GradingResult`, …)
//! - [`context`] — `RunContext` detection from parsed flags / environment
//! - [`runtime`] — runtime helpers (git spawning)
//!
//! The submodules are re-exported flat here so downstream code writes
//! `crate::core::Eval` rather than `crate::core::types::Eval`, matching how the
//! TypeScript original imported from the `core/` barrel.

pub mod context;
pub mod runtime;
pub mod types;

pub use context::{ContextError, DetectInput, Harness, RunContext, detect_run_context};
pub use runtime::{GitOutput, run_git};
pub use types::*;
