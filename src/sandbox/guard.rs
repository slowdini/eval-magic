//! Shared guard-hook plumbing.
//!
//! The harness-specific verdict shaping lives with each harness's guard module
//! (`crate::adapters::<harness>::guard`): the CLI handler reads the hook
//! payload from stdin and the marker path from argv, and the harness guard
//! calls [`parse_tool_call`] + `decide` to evaluate it. Both layers fail open —
//! a malformed payload or unreadable marker yields "allow", so the guard can
//! never brick a session.

use std::path::Path;

use serde_json::{Map, Value};

use super::decide::GuardMarker;

/// Read and parse the guard marker. Missing or unparseable → `None` (the guard
/// then allows everything — fail open).
pub fn read_marker(path: &Path) -> Option<GuardMarker> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Extract the tool name + input from a PreToolUse hook payload (the JSON the
/// harness sends on stdin). `None` for an empty or malformed payload — treated
/// as allow by every caller.
pub(crate) fn parse_tool_call(payload: &str) -> Option<(String, Value)> {
    let trimmed = payload.trim();
    let parsed: Value =
        serde_json::from_str(if trimmed.is_empty() { "{}" } else { trimmed }).ok()?;

    let tool_name = parsed
        .get("tool_name")
        .and_then(Value::as_str)
        .or_else(|| {
            parsed
                .get("tool")
                .and_then(Value::as_object)
                .and_then(|tool| tool.get("name"))
                .and_then(Value::as_str)
        })
        .unwrap_or("")
        .to_string();

    let tool_input = merge_top_level_files(
        parsed
            .get("tool_input")
            .or_else(|| parsed.get("input"))
            .cloned()
            .unwrap_or(Value::Null),
        &parsed,
    );

    Some((tool_name, tool_input))
}

fn merge_top_level_files(input: Value, parsed: &Value) -> Value {
    let Some(files) = parsed.get("files") else {
        return input;
    };

    match input {
        Value::Object(mut obj) => {
            obj.entry("files").or_insert_with(|| files.clone());
            Value::Object(obj)
        }
        Value::Null => {
            let mut obj = Map::new();
            obj.insert("files".to_string(), files.clone());
            Value::Object(obj)
        }
        other => other,
    }
}
