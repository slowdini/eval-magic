//! Baseline management and workspace cleanup.
//!
//! The whole workspace-artifact lifecycle — snapshot, promote, teardown —
//! lives in one module.

pub mod promote;
pub mod snapshot;
pub mod teardown;

pub use promote::{NotesStatus, PromoteOptions, PromoteResult, promote_baseline};
pub use snapshot::snapshot;
pub use teardown::{
    KeptIteration, PROMOTED_MARKER, SNAPSHOT_META, WorkspaceCleanupSummary, cleanup_workspace,
};

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, SecondsFormat};
use serde::Serialize;

/// A recoverable failure while managing workspace artifacts. Library-side
/// convention (mirrors `pipeline::PipelineError`); the CLI boundary maps it to
/// `anyhow`.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// A user-facing failure with a ready-to-display message.
    #[error("{0}")]
    Message(String),
    /// Filesystem IO failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// JSON parse/serialize failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// The current wall clock as `2026-06-08T12:00:00.000Z` (the `promoted_at`
/// stamp). `chrono` ships without its `clock` feature, so the instant comes
/// from `std::time`. Mirrors the per-module precedent (`sandbox::now_ms`,
/// `pipeline::io::now_iso8601`).
pub(crate) fn now_iso8601() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Write `value` to `path` as 2-space-pretty JSON with a trailing newline —
/// the stable on-disk format for every artifact this binary writes.
pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), WorkspaceError> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(path, text)?;
    Ok(())
}
