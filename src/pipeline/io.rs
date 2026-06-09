//! Shared JSON read/write helpers for the pipeline stages.
//!
//! Every stage serializes artifacts the same way eval-runner did:
//! `JSON.stringify(value, null, 2) + "\n"` — pretty-printed, two-space indent,
//! one trailing newline. `serde_json`'s `preserve_order` feature keeps object key
//! order stable so outputs diff cleanly against the TypeScript original.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, SecondsFormat};
use serde::Serialize;

use crate::pipeline::error::PipelineError;

/// Write `value` to `path` as pretty JSON with a trailing newline.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), PipelineError> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(path, text)?;
    Ok(())
}

/// The current wall clock as `2026-06-08T12:00:00.000Z`, matching JS
/// `new Date().toISOString()` — the `generated` stamp every report carries.
/// chrono ships without its `clock` feature, so the instant comes from
/// `std::time` and is formatted via chrono.
pub fn now_iso8601() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}
