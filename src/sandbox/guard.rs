//! Guard hook evaluation. Ports the logic of `src/sandbox/guard.ts`.
//!
//! eval-runner shipped this as a standalone Node script that the PreToolUse hook
//! invoked by path. Here it is library logic behind the binary's hidden `guard`
//! subcommand: the CLI handler reads the hook payload from stdin and the marker
//! path from argv, calls [`guard_decision`], and writes any deny verdict to
//! stdout. Both layers fail open — a malformed payload or unreadable marker
//! yields "allow", so the guard can never brick a session.

use std::path::Path;

use serde_json::{Value, json};

use super::decide::{GuardMarker, decide};
use super::now_ms;

/// Read and parse the guard marker. Missing or unparseable → `None` (the guard
/// then allows everything), matching eval-runner's fail-open `readMarker`.
pub fn read_marker(path: &Path) -> Option<GuardMarker> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Evaluate a PreToolUse hook `payload` (the JSON the harness sends on stdin)
/// against `marker`. Returns the serialized deny verdict to print on stdout when
/// the call is blocked, or `None` to allow (print nothing). An empty or malformed
/// payload is treated as allow.
pub fn guard_decision(payload: &str, marker: Option<GuardMarker>) -> Option<String> {
    let trimmed = payload.trim();
    let parsed: Value =
        serde_json::from_str(if trimmed.is_empty() { "{}" } else { trimmed }).ok()?;

    let tool_name = parsed
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let tool_input = parsed.get("tool_input").cloned().unwrap_or(Value::Null);

    let decision = decide(tool_name, &tool_input, marker.as_ref(), now_ms());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A live marker (active, no expiry → unexpired) scoped to one root.
    fn marker() -> GuardMarker {
        GuardMarker {
            active: Some(true),
            allowed_roots: Some(vec!["/work/skills-workspace".to_string()]),
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
    fn empty_or_malformed_payload_fails_open() {
        assert_eq!(guard_decision("", Some(marker())), None);
        assert_eq!(guard_decision("not json", Some(marker())), None);
    }
}
