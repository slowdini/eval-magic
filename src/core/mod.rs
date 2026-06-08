//! Shared kernel used by nearly every other module.
//!
//! Mirrors `src/core/` in eval-runner:
//! - [`types`]   — domain types (`Eval`, `RunRecord`, `Assertion`, `GradingResult`, …)
//! - [`context`] — `RunContext` detection from CLI flags / environment
//! - [`runtime`] — runtime & path helpers (git spawning, module path resolution)
//!
//! TODO(port): Phase 1 — see rewrite-roadmap.md.

pub mod context;
pub mod runtime;
pub mod types;
