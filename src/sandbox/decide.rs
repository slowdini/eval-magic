//! The guard arbiter.
//!
//! [`decide`] is the single decision point the armed PreToolUse hook consults:
//! given a tool call and the on-disk guard marker, it allows or denies. Writes
//! outside every allowed root and un-scoped Bash mutations are denied; everything
//! else — all read tools, and the orchestrator's own in-sandbox writes — is
//! allowed. When the guard is not armed, every call is allowed.

use chrono::DateTime;
use serde::Deserialize;
use serde_json::Value;

use super::policy::{classify_bash, is_under_any, is_write_tool, path_arg};

/// The marker file (`<stageRoot>/.claude/skills/.slow-powers-eval-guard.json`)
/// that arms the guard. The guard is a no-op unless this file exists, is active,
/// and has not expired — so a crashed run that never tore the hook down can't
/// silently block writes in the user's next interactive session.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardMarker {
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub allowed_roots: Option<Vec<String>>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// The outcome of [`decide`]: allow, or deny with a human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardDecision {
    pub allow: bool,
    pub reason: Option<String>,
}

impl GuardDecision {
    fn allow() -> Self {
        Self {
            allow: true,
            reason: None,
        }
    }

    fn deny(reason: String) -> Self {
        Self {
            allow: false,
            reason: Some(reason),
        }
    }
}

/// True when the marker is active and unexpired at `now_ms` (epoch milliseconds).
fn armed(marker: Option<&GuardMarker>, now_ms: i64) -> bool {
    let Some(marker) = marker else {
        return false;
    };
    if marker.active != Some(true) {
        return false;
    }
    if let Some(expires_at) = &marker.expires_at {
        match DateTime::parse_from_rfc3339(expires_at) {
            Ok(exp) if exp.timestamp_millis() <= now_ms => return false,
            // An unparseable timestamp can't prove expiry; treat as unexpired,
            // matching TS where `Date.parse` of a present-but-bad value is NaN
            // and `NaN <= now` is false.
            _ => {}
        }
    }
    true
}

/// Decide whether a tool call should be allowed while the eval guard is armed.
///
/// `tool_input` is the harness-supplied argument object. `now_ms` is the current
/// time in epoch milliseconds (parameterized for testability; callers pass the
/// real clock).
pub fn decide(
    tool_name: &str,
    tool_input: &Value,
    marker: Option<&GuardMarker>,
    now_ms: i64,
) -> GuardDecision {
    if !armed(marker, now_ms) {
        return GuardDecision::allow();
    }
    let roots = marker
        .and_then(|m| m.allowed_roots.clone())
        .unwrap_or_default();
    let repo_root = std::env::current_dir().unwrap_or_default();

    if is_write_tool(tool_name) {
        if let Some(p) = path_arg(tool_input)
            && !is_under_any(p, &roots, &repo_root)
        {
            return GuardDecision::deny(format!(
                "eval guard: {tool_name} to {p} is outside the eval sandbox (allowed: {})",
                roots.join(", ")
            ));
        }
        return GuardDecision::allow();
    }

    if tool_name == "Bash" {
        let command = tool_input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(reason) = classify_bash(command, &roots) {
            return GuardDecision::deny(format!(
                "eval guard: blocked Bash ({reason}) — runs outside the eval sandbox"
            ));
        }
    }

    GuardDecision::allow()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::now_ms;
    use serde_json::json;

    const ROOTS: [&str; 2] = ["/work/skills-workspace", "/work/.claude/skills"];

    /// An RFC3339 timestamp `offset_ms` from now — `future`/`past` bracket the
    /// current wall clock used by `decide`.
    fn rfc3339(offset_ms: i64) -> String {
        DateTime::from_timestamp_millis(now_ms() + offset_ms)
            .unwrap()
            .to_rfc3339()
    }

    fn future() -> String {
        rfc3339(60_000)
    }

    fn past() -> String {
        rfc3339(-60_000)
    }

    /// A live marker (active, unexpired, the standard roots), overridable per field.
    fn marker() -> GuardMarker {
        GuardMarker {
            active: Some(true),
            allowed_roots: Some(ROOTS.iter().map(|s| s.to_string()).collect()),
            expires_at: Some(future()),
        }
    }

    fn decide_now(tool: &str, input: Value, m: Option<&GuardMarker>) -> GuardDecision {
        decide(tool, &input, m, now_ms())
    }

    #[test]
    fn allows_everything_when_marker_is_null() {
        let d = decide_now("Write", json!({ "file_path": "/etc/passwd" }), None);
        assert!(d.allow);
    }

    #[test]
    fn allows_everything_when_marker_is_inactive_or_expired() {
        let inactive = GuardMarker {
            active: Some(false),
            ..marker()
        };
        assert!(
            decide_now(
                "Write",
                json!({ "file_path": "/etc/passwd" }),
                Some(&inactive)
            )
            .allow
        );

        let expired = GuardMarker {
            expires_at: Some(past()),
            ..marker()
        };
        assert!(
            decide_now(
                "Write",
                json!({ "file_path": "/etc/passwd" }),
                Some(&expired)
            )
            .allow
        );
    }

    #[test]
    fn allows_a_write_under_an_allowed_root() {
        let d = decide_now(
            "Write",
            json!({ "file_path": "/work/skills-workspace/x/outputs/a.md" }),
            Some(&marker()),
        );
        assert!(d.allow);
    }

    #[test]
    fn denies_a_write_outside_all_allowed_roots() {
        let d = decide_now(
            "Edit",
            json!({ "file_path": "/work/runner/run.ts" }),
            Some(&marker()),
        );
        assert!(!d.allow);
        assert!(d.reason.unwrap().to_lowercase().contains("outside"));
    }

    #[test]
    fn denies_an_install_command() {
        let d = decide_now(
            "Bash",
            json!({ "command": "npm install left-pad" }),
            Some(&marker()),
        );
        assert!(!d.allow);
        assert!(d.reason.unwrap().to_lowercase().contains("install"));
    }

    #[test]
    fn allows_a_bash_command_scoped_to_an_allowed_root() {
        let d = decide_now(
            "Bash",
            json!({ "command": "echo hi > /work/skills-workspace/x/outputs/log" }),
            Some(&marker()),
        );
        assert!(d.allow);
    }

    #[test]
    fn allows_non_mutating_bash_and_read_tools() {
        assert!(decide_now("Bash", json!({ "command": "ls -la /" }), Some(&marker())).allow);
        assert!(
            decide_now(
                "Read",
                json!({ "file_path": "/etc/passwd" }),
                Some(&marker())
            )
            .allow
        );
    }

    #[test]
    fn denies_git_worktree_add() {
        let d = decide_now(
            "Bash",
            json!({ "command": "git worktree add ../wt -b scratch" }),
            Some(&marker()),
        );
        assert!(!d.allow);
        assert!(d.reason.unwrap().to_lowercase().contains("worktree"));
    }

    #[test]
    fn denies_bash_that_creates_a_path_under_dot_claude_via_non_redirect_verb() {
        assert!(
            !decide_now(
                "Bash",
                json!({ "command": "mkdir -p .claude/foo" }),
                Some(&marker())
            )
            .allow
        );
        assert!(
            !decide_now(
                "Bash",
                json!({ "command": "cp out.txt .claude/bar" }),
                Some(&marker())
            )
            .allow
        );
    }

    #[test]
    fn denies_bash_that_creates_a_bare_skills_dir() {
        assert!(
            !decide_now(
                "Bash",
                json!({ "command": "mkdir skills" }),
                Some(&marker())
            )
            .allow
        );
        assert!(
            !decide_now(
                "Bash",
                json!({ "command": "cp -r src ./skills" }),
                Some(&marker())
            )
            .allow
        );
    }

    #[test]
    fn still_allows_reads_of_dot_claude_with_no_create_verb() {
        assert!(
            decide_now(
                "Bash",
                json!({ "command": "cat .claude/settings.json" }),
                Some(&marker())
            )
            .allow
        );
        assert!(decide_now("Bash", json!({ "command": "ls .claude" }), Some(&marker())).allow);
    }

    #[test]
    fn allows_a_create_scoped_to_the_dot_claude_skills_staging_root() {
        let d = decide_now(
            "Bash",
            json!({ "command": "mkdir -p /work/.claude/skills/staged-x" }),
            Some(&marker()),
        );
        assert!(d.allow);
    }

    #[test]
    fn does_not_flag_skills_workspace_as_a_bare_skills_write() {
        let d = decide_now(
            "Bash",
            json!({ "command": "mkdir -p /work/skills-workspace/x/outputs" }),
            Some(&marker()),
        );
        assert!(d.allow);
    }
}
