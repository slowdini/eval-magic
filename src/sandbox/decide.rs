//! The guard arbiter.
//!
//! [`decide`] is the single decision point the armed PreToolUse hook consults:
//! given a tool call and the on-disk guard marker, it allows or denies. Writes
//! outside every allowed root and recognized Bash targets that escape those roots
//! are denied; everything else — all read tools, and the orchestrator's own
//! in-sandbox writes — is allowed. When the guard is not armed, every call is
//! allowed.

use chrono::DateTime;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

use crate::core::GuardPolicyConfig;
use crate::core::fs::artifact_path;

use super::command_policy::COMMAND_POLICY_REASON;
use super::policy::{
    OUTPUT_REDIRECTION_REASON, apply_patch_paths, classify_bash_denials, is_patch_tool,
    is_shell_tool, is_under_any, is_write_tool, path_arg, resolve_path,
};

/// Prefix every guard denial reason carries. Harnesses surface the reason back
/// to the agent verbatim, so it also reaches the transcript — which is what lets
/// ingest tell a guard block apart from a harness permission refusal instead of
/// reporting the same denial twice.
pub const GUARD_REASON_PREFIX: &str = "eval guard: ";

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
    #[serde(default)]
    pub guard_policy: Option<GuardPolicyConfig>,
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

fn scratch_hint(roots: &[String]) -> String {
    roots.first().map_or_else(String::new, |root| {
        format!(
            ". For temporary or scratch files, use {}.",
            artifact_path(&Path::new(root).join(super::TASK_SCRATCH_DIR))
        )
    })
}

/// The allowed roots as the deny reason names them. Rendered the same way as
/// the scratch hint beside it, so one sentence never shows the agent a root in
/// one spelling and a directory under it in another.
fn allowed_roots_hint(roots: &[String]) -> String {
    roots
        .iter()
        .map(|root| artifact_path(Path::new(root)))
        .collect::<Vec<_>>()
        .join(", ")
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
    let default_policy = GuardPolicyConfig::default();
    let guard_policy = marker
        .and_then(|m| m.guard_policy.as_ref())
        .unwrap_or(&default_policy);

    if is_write_tool(tool_name) {
        if let Some(p) = path_arg(tool_input)
            && !is_under_any(p, &roots, invocation_cwd)
        {
            return GuardEvaluation::deny(
                format!(
                    "{GUARD_REASON_PREFIX}{tool_name} to {p} is outside the eval sandbox (allowed: {}){}",
                    allowed_roots_hint(&roots),
                    scratch_hint(&roots),
                ),
                vec![artifact_path(&resolve_path(p, invocation_cwd))],
            );
        }
        return GuardEvaluation::allow();
    }

    if is_patch_tool(tool_name) {
        let paths = apply_patch_paths(tool_input);
        if paths.is_empty() {
            return GuardEvaluation::deny(
                format!(
                    "{GUARD_REASON_PREFIX}blocked {tool_name} because no patch target path could \
                     be determined"
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
                .map(|target| artifact_path(&resolve_path(target, invocation_cwd)))
                .collect();
            return GuardEvaluation::deny(
                format!(
                    "{GUARD_REASON_PREFIX}{tool_name} target {path} is outside the eval sandbox \
                     (allowed: {}){}",
                    allowed_roots_hint(&roots),
                    scratch_hint(&roots),
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
        let denials = classify_bash_denials(command, &roots, invocation_cwd, guard_policy);
        if !denials.is_empty() {
            // A containment denial stays a self-contained clause when the
            // command policy also denies, so one verdict can name every
            // blocking layer instead of sending the agent to fix a problem
            // that cannot unblock the command.
            let clause = |reason: &str| {
                if reason == COMMAND_POLICY_REASON {
                    reason.to_string()
                } else {
                    format!("{reason} — runs outside the eval sandbox")
                }
            };
            let verdict = if let [only] = denials.as_slice() {
                let boundary = if only.reason == COMMAND_POLICY_REASON {
                    ""
                } else {
                    " — runs outside the eval sandbox"
                };
                format!("({}){boundary}", only.reason)
            } else {
                let clauses = denials
                    .iter()
                    .map(|denial| clause(denial.reason))
                    .collect::<Vec<_>>()
                    .join("; ");
                format!("({clauses})")
            };
            let hint = if denials
                .iter()
                .any(|denial| denial.reason == OUTPUT_REDIRECTION_REASON)
            {
                scratch_hint(&roots)
            } else {
                String::new()
            };
            let resolved_targets = denials
                .into_iter()
                .flat_map(|denial| denial.resolved_targets)
                .collect();
            return GuardEvaluation::deny(
                format!("{GUARD_REASON_PREFIX}blocked {tool_name} {verdict}{hint}"),
                resolved_targets,
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
            guard_policy: None,
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
    fn outside_file_write_points_temporary_work_to_task_scratch() {
        let d = decide_now(
            "Write",
            json!({ "file_path": "/tmp/repro.rs" }),
            Some(&marker()),
        );

        assert_eq!(
            d.reason.unwrap(),
            "eval guard: Write to /tmp/repro.rs is outside the eval sandbox \
             (allowed: /work/.eval-magic, /work/.claude/skills). For temporary or scratch files, \
             use /work/.eval-magic/tmp."
        );
    }

    #[test]
    fn denies_an_install_command_from_outside_the_guarded_environment() {
        let d = decide_with_cwd(
            "Bash",
            &json!({ "command": "npm install left-pad" }),
            Some(&marker()),
            now_ms(),
            Path::new("/outside/project"),
        )
        .decision;
        assert!(!d.allow);
        let reason = d.reason.unwrap();
        assert!(reason.to_lowercase().contains("install"));
        assert!(!reason.contains("temporary or scratch"));
    }

    #[test]
    fn marker_command_policy_allows_a_configured_tool() {
        let marker: GuardMarker = serde_json::from_value(json!({
            "active": true,
            "allowedRoots": ["/work/.eval-magic/task"],
            "guardPolicy": { "allow_tools": ["cargo"] }
        }))
        .unwrap();

        let d = decide_with_cwd(
            "Bash",
            &json!({ "command": "cargo build --release" }),
            Some(&marker),
            now_ms(),
            Path::new("/work/.eval-magic/task"),
        )
        .decision;

        assert!(d.allow, "{:?}", d.reason);

        let denied = decide_with_cwd(
            "Bash",
            &json!({ "command": "npm install" }),
            Some(&marker),
            now_ms(),
            Path::new("/work/.eval-magic/task"),
        )
        .decision;
        assert_eq!(
            denied.reason.as_deref(),
            Some("eval guard: blocked Bash (command not allowed by eval guard policy)")
        );
    }

    /// Issue #297: a command both layers would deny must get one verdict that
    /// names both reasons. Naming only the redirect (with its actionable
    /// scratch hint) sends the agent to fix a problem that cannot unblock the
    /// command, because the command policy was already denying it.
    #[test]
    fn a_bash_denial_names_every_blocking_layer_in_one_verdict() {
        let marker: GuardMarker = serde_json::from_value(json!({
            "active": true,
            "allowedRoots": ["/work/.eval-magic/task"],
            "guardPolicy": { "allow_tools": ["cargo"] }
        }))
        .unwrap();

        let denied = decide_with_cwd(
            "Bash",
            &json!({ "command": "npm run dev > /tmp/dev-server.log 2>&1 &" }),
            Some(&marker),
            now_ms(),
            Path::new("/work/.eval-magic/task"),
        );

        assert!(!denied.decision.allow);
        let reason = denied.decision.reason.unwrap();
        assert!(reason.contains("output redirection to a file"), "{reason}");
        assert!(
            reason.contains("command not allowed by eval guard policy"),
            "{reason}"
        );
        assert!(
            reason.ends_with("For temporary or scratch files, use /work/.eval-magic/task/tmp."),
            "{reason}"
        );
        assert_eq!(
            denied.resolved_targets,
            vec!["/tmp/dev-server.log".to_string()]
        );
    }

    #[test]
    fn a_bash_denial_from_containment_alone_keeps_the_single_reason_verdict() {
        let d = decide_now(
            "Bash",
            json!({ "command": "echo hi > /tmp/out.log" }),
            Some(&marker()),
        );

        assert_eq!(
            d.reason.as_deref(),
            Some(
                "eval guard: blocked Bash (output redirection to a file) \
                 — runs outside the eval sandbox. For temporary or scratch files, use \
                 /work/.eval-magic/tmp."
            )
        );
    }

    #[test]
    fn allows_bash_with_an_in_bounds_redirect() {
        let d = decide_now(
            "Bash",
            json!({ "command": "echo hi > /work/.eval-magic/x/outputs/log" }),
            Some(&marker()),
        );
        assert!(d.allow);
    }

    #[test]
    fn allows_an_in_bounds_redirect_when_heredoc_body_contains_shell_like_syntax() {
        let d = decide_with_cwd(
            "Bash",
            &json!({
                "command": "mkdir -p tmp && cat > tmp/verify.ts <<'EOF'\n\
                    const handlers: [string, (body: unknown) => Response][] = [];\n\
                    git push origin main\n\
                    EOF\n\
                    bun run tmp/verify.ts"
            }),
            Some(&marker()),
            now_ms(),
            Path::new("/work/.eval-magic"),
        );

        assert!(d.decision.allow, "{:?}", d.decision.reason);
    }

    #[test]
    fn outside_shell_redirection_points_temporary_work_to_task_scratch() {
        for command in [
            "printf done > /tmp/repro.log",
            "printf done > \"$DYNAMIC_TARGET\"",
        ] {
            let d = decide_now("Bash", json!({ "command": command }), Some(&marker()));

            assert!(
                d.reason
                    .unwrap()
                    .ends_with("For temporary or scratch files, use /work/.eval-magic/tmp."),
                "{command}"
            );
        }
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
    fn armed_guard_allows_local_git_but_denies_remote_git() {
        let cwd = Path::new("/work/.eval-magic/task");
        let local = decide_with_cwd(
            "Bash",
            &json!({ "command": "git add . && git commit -m done" }),
            Some(&marker()),
            now_ms(),
            cwd,
        );
        assert!(local.decision.allow, "{:?}", local.decision.reason);

        let remote = decide_with_cwd(
            "Bash",
            &json!({ "command": "git push /work/.eval-magic/task" }),
            Some(&marker()),
            now_ms(),
            cwd,
        );
        assert!(!remote.decision.allow);
        assert!(remote.decision.reason.unwrap().contains("remote"));

        let unguarded = decide_with_cwd(
            "Bash",
            &json!({ "command": "git push origin main" }),
            None,
            now_ms(),
            cwd,
        );
        assert!(unguarded.decision.allow);
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
    fn outside_patch_target_points_temporary_work_to_task_scratch() {
        let d = decide_now(
            "apply_patch",
            json!({ "files": ["/tmp/repro.patch"] }),
            Some(&marker()),
        );

        assert!(
            d.reason
                .unwrap()
                .ends_with("For temporary or scratch files, use /work/.eval-magic/tmp.")
        );
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
    fn allows_ordinary_filesystem_commands_inside_the_guarded_environment() {
        let marker = marker();
        let cwd = Path::new("/work/.eval-magic/task");
        for command in [
            "mkdir -p .claude/foo",
            "cp out.txt .claude/bar",
            "mkdir skills",
            "cp -r src ./skills",
            "mkdir -p .codex/foo",
            "cp hooks.json .codex/hooks.json",
            "mkdir -p .agents/foo",
            "touch .opencode/opencode.json",
        ] {
            let result = decide_with_cwd(
                "Bash",
                &json!({ "command": command }),
                Some(&marker),
                now_ms(),
                cwd,
            );
            assert!(
                result.decision.allow,
                "{command} should be allowed: {:?}",
                result.decision.reason
            );
        }
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
}
