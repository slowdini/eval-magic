//! Shared kernel used by nearly every other module.
//!
//! - [`types`]        — domain types (`Eval`, `RunRecord`, `Assertion`, `GradingResult`, …)
//! - [`context`]      — `RunContext` detection from parsed flags / environment
//! - [`capabilities`] — per-harness run-option capabilities
//! - [`runtime`]      — runtime helpers (git spawning)
//!
//! The submodules are re-exported flat here so downstream code writes
//! `crate::core::Eval` rather than `crate::core::types::Eval`.

pub mod capabilities;
pub mod context;
pub mod runtime;
pub mod types;

pub use capabilities::{HarnessRunCapabilities, capabilities_for};
pub use context::{ContextError, DetectInput, Harness, RunContext, detect_run_context};
pub use runtime::{GitOutput, run_git};
pub use types::*;
