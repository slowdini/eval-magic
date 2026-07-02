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

use std::fs;
use std::path::Path;

use serde::Serialize;

pub mod dispatch;
pub mod fixtures;
pub mod grouping;
pub mod orchestrate;
pub mod runbook;
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

/// Write `value` as 2-space-pretty JSON with a trailing newline, matching the
/// shared writer used across the other modules (`sandbox`/`pipeline`).
pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), RunError> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    std::fs::write(path, text)?;
    Ok(())
}

/// Copy a file or (recursively) a directory from `src` to `dst`.
pub(crate) fn copy_entry(src: &Path, dst: &Path) -> Result<(), RunError> {
    if fs::metadata(src)?.is_dir() {
        copy_dir_recursive(src, dst)
    } else {
        fs::copy(src, dst)?;
        Ok(())
    }
}

/// Recursively copy `src` into `dst` (creating `dst`). Mirrors the private
/// `copy_dir_recursive` in `workspace/snapshot.rs:159`, but returns [`RunError`]
/// (the workspace one is private and returns `WorkspaceError`).
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), RunError> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
