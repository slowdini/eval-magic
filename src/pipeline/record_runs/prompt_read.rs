//! The prompt-read guard: whether a dispatch's transcript shows the agent
//! trying to read its dispatch prompt and getting an error instead.
//!
//! A dispatch that never received its instructions still exits 0 and still
//! emits a final message, so nothing downstream distinguishes it from a real
//! run. `record_runs` consults this to skip it rather than record a silent
//! no-op as data.

use std::fs;

use serde_json::Value;

use crate::adapters::TranscriptSummary;
use crate::core::fs::normalize_separators;

/// Positive evidence that the agent tried to read its dispatch prompt and
/// failed: the transcript has a tool call referencing `prompt_path`, yet no such
/// call returned the prompt's content (its distinctive first-line `sentinel`).
///
/// A run that never references the prompt path is NOT flagged — absence is not
/// proof of failure (the agent can receive the prompt another way),
/// and requiring positive evidence keeps the check free of false positives.
/// An invocation without a string result is not judged either: transcript
/// readers that leave results unjoined (the declarative extract tier) carry no
/// delivery evidence, and treating that as failure flags every successful read.
/// Returns `false` when `sentinel` is empty (the prompt file was missing or
/// unreadable, so the read cannot be judged).
pub(super) fn prompt_read_failed(
    summary: &TranscriptSummary,
    prompt_path: &str,
    sentinel: &str,
) -> bool {
    if sentinel.is_empty() {
        return false;
    }
    // The dispatch spells the path as wire format while the transcript echoes
    // the agent's own host spelling, so neither side can be matched as-is.
    let needle = normalize_separators(prompt_path);
    let mut referenced = false;
    let mut delivered = false;
    for inv in &summary.tool_invocations {
        let mentions_prompt = inv
            .args
            .as_ref()
            .is_some_and(|a| args_name_path(a, &needle));
        if !mentions_prompt {
            continue;
        }
        let Some(result) = inv.result.as_ref().and_then(Value::as_str) else {
            continue;
        };
        referenced = true;
        if result.contains(sentinel) {
            delivered = true;
        }
    }
    referenced && !delivered
}

/// True when any string leaf of a tool call's `args` names `needle`, which the
/// caller has already separator-normalized.
///
/// Walks the leaves rather than searching `args.to_string()`, for two reasons:
/// serializing to JSON escapes a Windows separator to `\\`, so a path never
/// appears in the serialized text as written; and the path can sit at any depth
/// — cline's `read_files` carries it at `files[].path`.
fn args_name_path(args: &Value, needle: &str) -> bool {
    match args {
        Value::String(text) => normalize_separators(text).contains(needle),
        Value::Array(items) => items.iter().any(|item| args_name_path(item, needle)),
        Value::Object(map) => map.values().any(|value| args_name_path(value, needle)),
        _ => false,
    }
}

/// The dispatch prompt's distinctive first non-empty line, used as the sentinel
/// for [`prompt_read_failed`]. Empty when the prompt file is missing/unreadable.
pub(super) fn prompt_sentinel(prompt_path: &str) -> String {
    if prompt_path.is_empty() {
        return String::new();
    }
    fs::read_to_string(prompt_path)
        .ok()
        .and_then(|p| {
            p.lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
        })
        .unwrap_or_default()
}
