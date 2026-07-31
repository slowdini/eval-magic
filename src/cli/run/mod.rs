//! The `run` orchestrator.
//!
//! Split into focused sub-orchestrators:
//!
//! - [`staging`] — staged-skill lifecycle (install/cleanup + sibling manifest).
//! - [`dispatch`] — dispatch-task and prompt assembly (`dispatch.json`).
//! - [`steps`] — the `ingest` / `finalize` fixed-order chains.
//! - [`orchestrate`] — `command_run`, the top-level orchestrator.
//!
//! The `snapshot` subcommand lives in [`crate::workspace::snapshot`] with the
//! rest of the workspace-artifact lifecycle, so it has no home here.

pub mod conversation;
pub mod dispatch;
pub mod fixtures;
#[cfg(test)]
mod golden_tests;
pub mod grouping;
pub mod orchestrate;
pub mod runbook;
mod scratch;
pub mod staging;
pub mod steps;
mod util;

/// A user-facing failure inside the `run` orchestrator. Mirrors
/// [`crate::pipeline::PipelineError`] / `WorkspaceError`: `Message` carries
/// bespoke ready-to-display strings, and the transparent variants forward the
/// underlying I/O, JSON, and schema-validation errors.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Validation(#[from] crate::validation::ValidationError),
}

impl RunError {
    /// Construct a [`RunError::Message`] from anything string-like.
    pub fn msg(text: impl Into<String>) -> Self {
        RunError::Message(text.into())
    }
}
