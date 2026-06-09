//! Shared JSON read/write helpers for the pipeline stages.
//!
//! Every stage serializes artifacts the same way eval-runner did:
//! `JSON.stringify(value, null, 2) + "\n"` — pretty-printed, two-space indent,
//! one trailing newline. `serde_json`'s `preserve_order` feature keeps object key
//! order stable so outputs diff cleanly against the TypeScript original.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::pipeline::error::PipelineError;

/// Write `value` to `path` as pretty JSON with a trailing newline.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), PipelineError> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(path, text)?;
    Ok(())
}
