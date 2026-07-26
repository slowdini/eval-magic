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
use std::path::Path;

use super::policy::{
    apply_patch_paths, classify_bash_with_cwd, is_patch_tool, is_shell_tool, is_under_any,
    is_write_tool, path_arg, resolve_path,
};

/// The staged marker file that arms the guard. The guard is a no-op unless this
/// file exists, is active, and has not expired — so a crashed run that never tore
/// the hook down can't silently block writes in the user's next interactive
/// session.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardMarker {
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub allowed_roots: Option<Vec<String>>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub denial_log_path: Option<String>,
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

/// Internal decision envelope carrying privacy-safe path evidence for denial
/// logging. The public [`GuardDecision`] API remains unchanged.
pub(crate) struct GuardEvaluation {
    pub decision: GuardDecision,
    pub resolved_targets: Vec<String>,
}

impl GuardEvaluation {
    fn allow() -> Self {
        Self {
            decision: GuardDecision::allow(),
            resolved_targets: Vec::new(),
        }
    }

    fn deny(reason: String, resolved_targets: Vec<String>) -> Self {
        Self {
            decision: GuardDecision::deny(reason),
            resolved_targets,
        }
    }
}

/// True when the marker is active and unexpired at `now_ms` (epoch milliseconds).
pub(crate) fn marker_is_armed(marker: Option<&GuardMarker>, now_ms: i64) -> bool {
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
    let cwd = std::env::current_dir().unwrap_or_default();
    decide_with_cwd(tool_name, tool_input, marker, now_ms, &cwd).decision
}

/// Cwd-aware implementation behind [`decide`]. Hook callers pass the cwd from
/// the invocation payload; legacy/public callers retain process-cwd behavior.
pub(crate) fn decide_with_cwd(
    tool_name: &str,
    tool_input: &Value,
    marker: Option<&GuardMarker>,
    now_ms: i64,
    invocation_cwd: &Path,
) -> GuardEvaluation {
    if !marker_is_armed(marker, now_ms) {
        return GuardEvaluation::allow();
    }
    let roots = marker
        .and_then(|m| m.allowed_roots.clone())
        .unwrap_or_default();

    if is_write_tool(tool_name) {
        if let Some(p) = path_arg(tool_input)
            && !is_under_any(p, &roots, invocation_cwd)
        {
            return GuardEvaluation::deny(
                format!(
                    "eval guard: {tool_name} to {p} is outside the eval sandbox (allowed: {})",
                    roots.join(", ")
                ),
                vec![resolve_path(p, invocation_cwd).display().to_string()],
            );
        }
        return GuardEvaluation::allow();
    }

    if is_patch_tool(tool_name) {
        let paths = apply_patch_paths(tool_input);
        if paths.is_empty() {
            return GuardEvaluation::deny(
                format!(
                    "eval guard: blocked {tool_name} because no patch target path could be \
                     determined"
                ),
                Vec::new(),
            );
        }
        if let Some(path) = paths
            .iter()
            .find(|p| !is_under_any(p, &roots, invocation_cwd))
        {
            let resolved_targets = paths
                .iter()
                .map(|target| resolve_path(target, invocation_cwd).display().to_string())
                .collect();
            return GuardEvaluation::deny(
                format!(
                    "eval guard: {tool_name} target {path} is outside the eval sandbox \
                     (allowed: {})",
                    roots.join(", ")
                ),
                resolved_targets,
            );
        }
        return GuardEvaluation::allow();
    }

    if is_shell_tool(tool_name) {
        let command = tool_input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(classification) = classify_bash_with_cwd(command, &roots, invocation_cwd) {
            return GuardEvaluation::deny(
                format!(
                    "eval guard: blocked {tool_name} ({}) — runs outside the eval sandbox",
                    classification.reason
                ),
                classification.resolved_targets,
            );
        }
    }

    GuardEvaluation::allow()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::now_ms;
    use serde_json::json;

    const ROOTS: [&str; 2] = ["/work/.eval-magic", "/work/.claude/skills"];

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
            denial_log_path: None,
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
            json!({ "file_path": "/work/.eval-magic/x/outputs/a.md" }),
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
            json!({ "command": "echo hi > /work/.eval-magic/x/outputs/log" }),
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
    fn denies_apply_patch_outside_allowed_roots() {
        let d = decide_now(
            "apply_patch",
            json!({ "files": ["/work/runner/src/lib.rs"] }),
            Some(&marker()),
        );
        assert!(!d.allow);
        assert!(d.reason.unwrap().contains("apply_patch"));
    }

    #[test]
    fn allows_apply_patch_inside_allowed_roots() {
        let d = decide_now(
            "apply_patch",
            json!({ "files": ["/work/.eval-magic/eval/outputs/out.md"] }),
            Some(&marker()),
        );
        assert!(d.allow);
    }

    #[test]
    fn denies_apply_patch_without_a_known_target() {
        let d = decide_now("apply_patch", json!({}), Some(&marker()));
        assert!(!d.allow);
        assert!(d.reason.unwrap().contains("no patch target"));
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
    fn denies_bash_that_creates_a_path_under_dot_codex_via_non_redirect_verb() {
        assert!(
            !decide_now(
                "Bash",
                json!({ "command": "mkdir -p .codex/foo" }),
                Some(&marker())
            )
            .allow
        );
        assert!(
            !decide_now(
                "Bash",
                json!({ "command": "cp evil.json .codex/hooks.json" }),
                Some(&marker())
            )
            .allow
        );
    }

    #[test]
    fn denies_bash_that_creates_a_path_under_dot_agents_via_non_redirect_verb() {
        assert!(
            !decide_now(
                "Bash",
                json!({ "command": "mkdir -p .agents/foo" }),
                Some(&marker())
            )
            .allow
        );
    }

    #[test]
    fn denies_bash_that_creates_a_path_under_dot_opencode_via_non_redirect_verb() {
        assert!(
            !decide_now(
                "Bash",
                json!({ "command": "touch .opencode/opencode.json" }),
                Some(&marker())
            )
            .allow
        );
    }

    #[test]
    fn still_allows_reads_of_other_harness_config_dirs_with_no_create_verb() {
        for command in [
            "cat .codex/hooks.json",
            "ls .agents",
            "cat .opencode/skills/x/SKILL.md",
        ] {
            assert!(
                decide_now("Bash", json!({ "command": command }), Some(&marker())).allow,
                "{command} should stay allowed"
            );
        }
    }

    #[test]
    fn allows_a_create_scoped_to_a_codex_skills_staging_root() {
        let codex_marker = GuardMarker {
            allowed_roots: Some(vec![
                "/work/.eval-magic".to_string(),
                "/work/.agents/skills".to_string(),
            ]),
            ..marker()
        };
        let d = decide_now(
            "Bash",
            json!({ "command": "mkdir -p /work/.agents/skills/staged-x" }),
            Some(&codex_marker),
        );
        assert!(d.allow);
    }

    #[test]
    fn does_not_flag_a_skills_prefixed_dir_as_a_bare_skills_write() {
        // A `skills`-prefixed path that is NOT an allowed root: the bare-`skills/`
        // heuristic only fires on a bare `skills` at a path boundary, so a
        // `skills-`-prefixed dir must not be flagged and the write is allowed.
        let d = decide_now(
            "Bash",
            json!({ "command": "mkdir -p /work/skills-data/x/outputs" }),
            Some(&marker()),
        );
        assert!(d.allow);
    }
}
