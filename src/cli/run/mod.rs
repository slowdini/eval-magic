//! The `run` orchestrator and its run-mode variants.
//!
//! Ports eval-runner's `src/cli/run.ts` (~1,593 LOC) — the single largest file
//! in the TypeScript original — split into focused sub-orchestrators rather than
//! ported as one module (the main code-quality win of the rewrite):
//!
//! - [`staging`] — staged-skill lifecycle (install/cleanup + sibling manifest).
//! - [`dispatch`] — dispatch-task and prompt assembly (`dispatch.json`).
//! - [`steps`] — the `ingest` / `finalize` fixed-order chains.
//! - [`orchestrate`] — `command_run`, the top-level orchestrator.
//!
//! The `snapshot` subcommand lived in `run.ts` but was ported to
//! [`crate::workspace::snapshot`] in phase 6, so it has no home here. `cli.ts`'s
//! manual dispatch/help has no counterpart — `clap` owns both (see
//! [`crate::cli`]).

use std::fs;
use std::path::Path;

use serde::Serialize;

pub mod dispatch;
pub mod orchestrate;
pub mod staging;
pub mod steps;

/// A user-facing failure inside the `run` orchestrator. Mirrors
/// [`crate::pipeline::PipelineError`] / `WorkspaceError`: `Message` carries the
/// bespoke strings the TS original passed to `die(...)`, and the transparent
/// variants forward the underlying I/O, JSON, and schema-validation errors.
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
    /// Construct a [`RunError::Message`] from anything string-like — the Rust
    /// equivalent of the TS `die("...")` call sites.
    pub fn msg(text: impl Into<String>) -> Self {
        RunError::Message(text.into())
    }
}

/// Write `value` as 2-space-pretty JSON with a trailing newline — byte-for-byte
/// what eval-runner's `JSON.stringify(value, null, 2) + "\n"` produced, matching
/// the shared writer used across the other modules (`sandbox`/`pipeline`).
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
