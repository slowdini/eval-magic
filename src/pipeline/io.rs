//! Shared timestamp helper for the pipeline stages.
//!
//! Artifact JSON writing lives in [`crate::core::fs::write_json`] — every stage
//! serializes the same way (pretty-printed, two-space indent, one trailing
//! newline), so the writer is shared crate-wide rather than per-module.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, SecondsFormat};

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
