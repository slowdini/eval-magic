//! Shared error type for the pipeline stages.
//!
//! The stages orchestrate filesystem IO, JSON (de)serialization, and schema
//! validation, so a stage failure can originate in any of those. `PipelineError`
//! unifies them behind one `thiserror` enum (the library-side convention; the CLI
//! boundary maps it to `anyhow`).

/// A recoverable failure while running a pipeline stage.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// A stage-specific failure with a ready-to-display message.
    #[error("{0}")]
    Message(String),
    /// Filesystem IO failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// JSON parse/serialize failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A produced artifact failed schema validation before write.
    #[error(transparent)]
    Validation(#[from] crate::validation::ValidationError),
}
