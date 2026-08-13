use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn extracts_approval_policy_denial_from_sibling_stderr_capture() {
    let dir = TempDir::new().unwrap();
    let events_path = dir.path().join("codex-events.jsonl");
    fs::write(&events_path, "{\"type\":\"turn.completed\"}\n").unwrap();
    fs::write(
        dir.path().join("codex-stderr.log"),
        concat!(
            "2026-07-30T06:06:21Z ERROR codex_core::tools::router: ",
            "error=exec_command failed for `/bin/zsh -lc pwd`: CreateProcess { ",
            "message: \"Rejected(\\\"approval required by policy, but AskForApproval is set ",
            "to Never\\\")\" }\n"
        ),
    )
    .unwrap();

    assert_eq!(
        parse_codex_permission_denials(&events_path).unwrap(),
        vec![crate::adapters::PermissionDenial {
            tool: "Bash".into(),
            reason: Some("approval required by policy, but AskForApproval is set to Never".into()),
            input_keys: vec!["command".into()],
        }]
    );
}

#[test]
fn extracts_explicit_rule_denial_without_copying_the_command_into_the_reason() {
    let dir = TempDir::new().unwrap();
    let events_path = dir.path().join("codex-events.jsonl");
    fs::write(
        dir.path().join("codex-stderr.log"),
        concat!(
            "2026-07-30T06:08:16Z ERROR codex_core::tools::router: ",
            "error=exec_command failed for `/bin/zsh -lc pwd`: CreateProcess { ",
            "message: \"Rejected(\\\"`/bin/zsh -lc pwd` rejected: ",
            "permission-denial probe\\\")\" }\n"
        ),
    )
    .unwrap();

    let denials = parse_codex_permission_denials(&events_path).unwrap();
    assert_eq!(
        denials,
        vec![crate::adapters::PermissionDenial {
            tool: "Bash".into(),
            reason: Some("permission-denial probe".into()),
            input_keys: vec!["command".into()],
        }]
    );
    assert!(!denials[0].reason.as_deref().unwrap().contains("/bin/zsh"));
}

#[test]
fn extracts_pre_tool_use_shell_denial_and_preserves_guard_attribution() {
    let dir = TempDir::new().unwrap();
    let events_path = dir.path().join("codex-events.jsonl");
    fs::write(
        dir.path().join("codex-stderr.log"),
        concat!(
            "2026-07-30T06:09:01Z ERROR codex_core::tools::router: ",
            "error=Command blocked by PreToolUse hook: ",
            "eval guard: probe denial. Command: pwd\n"
        ),
    )
    .unwrap();

    assert_eq!(
        parse_codex_permission_denials(&events_path).unwrap(),
        vec![crate::adapters::PermissionDenial {
            tool: "Bash".into(),
            reason: Some("eval guard: probe denial".into()),
            input_keys: vec!["command".into()],
        }]
    );
}

#[test]
fn extracts_pre_tool_use_patch_denial_without_copying_patch_payload() {
    let dir = TempDir::new().unwrap();
    let events_path = dir.path().join("codex-events.jsonl");
    fs::write(
        dir.path().join("codex-stderr.log"),
        concat!(
            "2026-07-30T06:10:10Z ERROR codex_core::tools::router: ",
            "error=Command blocked by PreToolUse hook: ",
            "eval guard: probe denial. Command: *** Begin Patch\n",
            "*** Update File: secret.txt\n",
            "@@\n",
            "+do not copy this payload\n",
            "*** End Patch\n"
        ),
    )
    .unwrap();

    let denials = parse_codex_permission_denials(&events_path).unwrap();
    assert_eq!(
        denials,
        vec![crate::adapters::PermissionDenial {
            tool: "apply_patch".into(),
            reason: Some("eval guard: probe denial".into()),
            input_keys: vec!["command".into()],
        }]
    );
    assert!(!format!("{denials:?}").contains("do not copy this payload"));
}

#[test]
fn ignores_rejection_lookalikes_outside_codex_tool_router_errors() {
    let dir = TempDir::new().unwrap();
    let events_path = dir.path().join("codex-events.jsonl");
    fs::write(
        dir.path().join("codex-stderr.log"),
        concat!(
            "application output: Rejected(\\\"approval required by policy, ",
            "but AskForApproval is set to Never\\\")\n"
        ),
    )
    .unwrap();

    assert_eq!(
        parse_codex_permission_denials(&events_path).unwrap(),
        Vec::new()
    );
}

#[test]
fn missing_or_unpaired_stderr_capture_produces_no_denials() {
    let dir = TempDir::new().unwrap();

    assert_eq!(
        parse_codex_permission_denials(&dir.path().join("codex-events.jsonl")).unwrap(),
        Vec::new()
    );
    fs::write(
        dir.path().join("stderr.log"),
        concat!(
            "2026-07-30T06:06:21Z ERROR codex_core::tools::router: ",
            "error=exec_command failed for `pwd`: CreateProcess { ",
            "message: \"Rejected(\\\"approval required by policy, but ",
            "AskForApproval is set to Never\\\")\" }\n"
        ),
    )
    .unwrap();
    assert_eq!(
        parse_codex_permission_denials(&dir.path().join("events.txt")).unwrap(),
        Vec::new()
    );
}

#[test]
fn ordinary_failed_command_events_and_process_errors_are_not_denials() {
    let dir = TempDir::new().unwrap();
    let events_path = dir.path().join("codex-events.jsonl");
    fs::write(
        &events_path,
        concat!(
            r#"{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"curl https://example.test","status":"failed","exit_code":6,"aggregated_output":"Could not resolve host"}}"#,
            "\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("codex-stderr.log"),
        concat!(
            "2026-07-30T06:06:21Z ERROR codex_core::tools::router: ",
            "error=exec_command failed for `missing`: CreateProcess { ",
            "message: \"No such file or directory\" }\n"
        ),
    )
    .unwrap();

    assert_eq!(
        parse_codex_permission_denials(&events_path).unwrap(),
        Vec::new()
    );
}
