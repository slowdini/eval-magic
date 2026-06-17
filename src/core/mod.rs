//! Shared kernel used by nearly every other module.
//!
//! - [`types`]    — domain types (`Eval`, `RunRecord`, `Assertion`, `GradingResult`, …)
//! - [`context`]  — `RunContext` detection from parsed flags / environment
//! - [`run_mode`] — dispatch mechanism (in-session vs. one-shot CLI)
//! - [`runtime`]  — runtime helpers (git spawning)
//!
//! The submodules are re-exported flat here so downstream code writes
//! `crate::core::Eval` rather than `crate::core::types::Eval`.

pub mod context;
pub mod run_mode;
pub mod runtime;
pub mod types;

pub use context::{ContextError, DetectInput, Harness, RunContext, detect_run_context};
pub use run_mode::{DispatchMechanism, HarnessRunCapabilities, capabilities_for, mechanism_for};
pub use runtime::{GitOutput, run_git};
pub use types::*;
