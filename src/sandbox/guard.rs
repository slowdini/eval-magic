//! Guard hook evaluation.
//!
//! Library logic behind the binary's hidden `guard` subcommand: the CLI handler
//! reads the hook payload from stdin and the marker path from argv, calls
//! [`guard_decision`], and writes any deny verdict to stdout. Both layers fail
//! open — a malformed payload or unreadable marker yields "allow", so the guard
//! can never brick a session.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::decide::{GuardMarker, decide};
use super::now_ms;

/// Read and parse the guard marker. Missing or unparseable → `None` (the guard
/// then allows everything — fail open).
pub fn read_marker(path: &Path) -> Option<GuardMarker> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Evaluate a PreToolUse hook `payload` (the JSON the harness sends on stdin)
/// against `marker`. Returns the serialized deny verdict to print on stdout when
/// the call is blocked, or `None` to allow (print nothing). An empty or malformed
/// payload is treated as allow.
pub fn guard_decision(payload: &str, marker: Option<GuardMarker>) -> Option<String> {
    let (tool_name, tool_input) = parse_tool_call(payload)?;

    let decision = decide(&tool_name, &tool_input, marker.as_ref(), now_ms());
    if decision.allow {
        return None;
    }
    Some(
        serde_json::to_string(&json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": decision.reason,
            }
        }))
        .expect("deny verdict serializes"),
    )
}

/// Codex's hook contract blocks by returning `{ "decision": "block", "reason":
/// "..." }` on stdout. Keep this separate from Claude Code's
/// `hookSpecificOutput` shape so both harnesses use their native conventions.
pub fn codex_guard_decision(payload: &str, marker: Option<GuardMarker>) -> Option<String> {
    let (tool_name, tool_input) = parse_tool_call(payload)?;
    let decision = decide(&tool_name, &tool_input, marker.as_ref(), now_ms());
    if decision.allow {
        return None;
    }
    Some(
        serde_json::to_string(&json!({
            "decision": "block",
            "reason": decision.reason,
        }))
        .expect("Codex block verdict serializes"),
    )
}

fn parse_tool_call(payload: &str) -> Option<(String, Value)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A live marker (active, no expiry → unexpired) scoped to one root.
    fn marker() -> GuardMarker {
        GuardMarker {
            active: Some(true),
            allowed_roots: Some(vec!["/work/.eval-magic".to_string()]),
            expires_at: None,
        }
    }

    #[test]
    fn allows_returns_none() {
        let payload = r#"{ "tool_name": "Read", "tool_input": { "file_path": "/etc/passwd" } }"#;
        assert_eq!(guard_decision(payload, Some(marker())), None);
    }

    #[test]
    fn deny_returns_pretooluse_deny_json() {
        let payload = r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#;
        let out = guard_decision(payload, Some(marker())).expect("should deny");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(
            v["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("outside")
        );
    }

    #[test]
    fn no_marker_allows_everything() {
        let payload = r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#;
        assert_eq!(guard_decision(payload, None), None);
    }

    #[test]
    fn codex_deny_returns_decision_block_json() {
        let payload = r#"{ "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": { "command": "npm install left-pad" } }"#;
        let out = codex_guard_decision(payload, Some(marker())).expect("should block");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("blocked Bash"));
    }

    #[test]
    fn codex_apply_patch_outside_allowed_roots_blocks() {
        let payload = r#"{ "hook_event_name": "PreToolUse", "tool_name": "apply_patch", "tool_input": { "files": ["/etc/passwd"] } }"#;
        let out = codex_guard_decision(payload, Some(marker())).expect("should block");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("apply_patch"));
    }

    #[test]
    fn codex_apply_patch_inside_allowed_roots_allows() {
        let payload = r#"{ "hook_event_name": "PreToolUse", "tool_name": "apply_patch", "tool_input": { "files": ["/work/.eval-magic/out.md"] } }"#;
        assert_eq!(codex_guard_decision(payload, Some(marker())), None);
    }

    #[test]
    fn empty_or_malformed_payload_fails_open() {
        assert_eq!(guard_decision("", Some(marker())), None);
        assert_eq!(guard_decision("not json", Some(marker())), None);
    }
}
