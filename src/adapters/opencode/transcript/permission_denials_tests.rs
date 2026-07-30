//! OpenCode permission-denial parsing from the event stream.
//!
//! OpenCode encodes a refused tool call as an ordinary `tool_use` event whose
//! `part.state.status` is `"error"` — there is no dedicated refusal channel.
//! The discriminator is the `state.error` string, which OpenCode itself authors
//! at the permission layer (before the tool body runs): an explicit deny rule
//! throws `PermissionDeniedError` with a fixed prefix, a headless reject throws
//! `PermissionRejectedError`/`PermissionCorrectedError`, and the eval write
//! guard throws the shared `eval guard: ` reason. Ordinary tool errors never
//! produce those strings, so matching them is not guessing from result text.
//! Permission-rule echoes and user correction feedback are stripped from the
//! reason; the guard reason is kept verbatim so the pipeline can attribute it.

use super::*;
use crate::sandbox::GUARD_REASON_PREFIX;
use serde_json::{Value, json};
use std::fs;
use tempfile::TempDir;

/// The stable prefix OpenCode's `PermissionDeniedError` always emits, verbatim,
/// before it appends the operator's ruleset as JSON.
const DENY_RULE_PREFIX: &str =
    "The user has specified a rule which prevents you from using this specific tool call.";
/// The common prefix of `PermissionRejectedError` and `PermissionCorrectedError`.
const REJECT_PREFIX: &str = "The user rejected permission to use this specific tool call";

/// A `tool_use` envelope wrapping a tool part whose state carries `status` and
/// `error`/`input`, mirroring `opencode run --format json`'s shape.
fn tool_use_error(tool: &str, state: Value) -> Value {
    json!({
        "type": "tool_use",
        "timestamp": 1_000,
        "sessionID": "ses_1",
        "part": {
            "id": "prt_1",
            "sessionID": "ses_1",
            "messageID": "msg_1",
            "type": "tool",
            "callID": "call_1",
            "tool": tool,
            "state": state,
        }
    })
}

fn denial(tool: &str, reason: &str, input_keys: &[&str]) -> crate::adapters::PermissionDenial {
    crate::adapters::PermissionDenial {
        tool: tool.into(),
        reason: Some(reason.into()),
        input_keys: input_keys.iter().map(|k| (*k).into()).collect(),
    }
}

#[test]
fn extracts_explicit_deny_rule_denial_from_the_event_stream() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("opencode-events.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n",
            tool_use_error(
                "bash",
                json!({
                    "status": "error",
                    "input": {"command": "pwd"},
                    "error": format!(
                        "{DENY_RULE_PREFIX} Here are some of the relevant rules \
                        [{{\"permission\":\"bash\",\"pattern\":\"pwd\",\"action\":\"deny\"}}]",
                    ),
                    "time": {"start": 900, "end": 1_000}
                })
            )
        ),
    )
    .unwrap();

    assert_eq!(
        parse_opencode_permission_denials(&path).unwrap(),
        vec![denial("bash", DENY_RULE_PREFIX, &["command"])]
    );
}

#[test]
fn strips_the_ruleset_json_so_operator_rules_do_not_bloat_the_reason() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("opencode-events.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n",
            tool_use_error(
                "bash",
                json!({
                    "status": "error",
                    "input": {"command": "ls -la"},
                    "error": format!(
                        "{DENY_RULE_PREFIX} Here are some of the relevant rules \
                        [{{\"permission\":\"*\",\"action\":\"allow\",\"pattern\":\"*\"}},\
                        {{\"permission\":\"bash\",\"pattern\":\"*\",\"action\":\"allow\"}},\
                        {{\"permission\":\"bash\",\"pattern\":\"ls *\",\"action\":\"deny\"}}]",
                    )
                })
            )
        ),
    )
    .unwrap();

    let denials = parse_opencode_permission_denials(&path).unwrap();
    assert_eq!(denials.len(), 1);
    let reason = denials[0].reason.as_deref().unwrap();
    assert_eq!(reason, DENY_RULE_PREFIX);
    assert!(
        !reason.contains("permission"),
        "ruleset JSON leaked: {reason}"
    );
    assert!(!reason.contains("bash"), "ruleset JSON leaked: {reason}");
}

#[test]
fn extracts_headless_reject_denial_and_normalizes_the_reason() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("opencode-events.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n",
            tool_use_error(
                "bash",
                json!({
                    "status": "error",
                    "input": {"command": "pwd"},
                    "error": format!("{REJECT_PREFIX}.")
                })
            )
        ),
    )
    .unwrap();

    assert_eq!(
        parse_opencode_permission_denials(&path).unwrap(),
        vec![denial("bash", &format!("{REJECT_PREFIX}."), &["command"])]
    );
}

#[test]
fn extracts_corrected_reject_and_drops_the_user_feedback() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("opencode-events.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n",
            tool_use_error(
                "bash",
                json!({
                    "status": "error",
                    "input": {"command": "pwd"},
                    "error": format!("{REJECT_PREFIX} with the following feedback: do not echo this")
                })
            )
        ),
    )
    .unwrap();

    let denials = parse_opencode_permission_denials(&path).unwrap();
    assert_eq!(denials.len(), 1);
    let reason = denials[0].reason.as_deref().unwrap();
    assert_eq!(reason, &format!("{REJECT_PREFIX}."));
    assert!(!reason.contains("do not echo"), "feedback leaked: {reason}");
}

#[test]
fn guard_blocks_surface_verbatim_with_the_guard_reason_prefix() {
    let reason = "eval guard: write to /etc/passwd is outside the eval sandbox";
    assert!(reason.starts_with(GUARD_REASON_PREFIX));
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("opencode-events.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n",
            tool_use_error(
                "edit",
                json!({
                    "status": "error",
                    "input": {"filePath": "/etc/passwd", "oldString": "a", "newString": "b"},
                    "error": reason
                })
            )
        ),
    )
    .unwrap();

    let denials = parse_opencode_permission_denials(&path).unwrap();
    assert_eq!(
        denials,
        vec![denial(
            "edit",
            reason,
            &["filePath", "newString", "oldString"]
        )]
    );
    assert!(
        denials[0]
            .reason
            .as_deref()
            .unwrap()
            .starts_with(GUARD_REASON_PREFIX)
    );
}

#[test]
fn records_sorted_input_keys_of_the_refused_call_omitting_values() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("opencode-events.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n",
            tool_use_error(
                "apply_patch",
                json!({
                    "status": "error",
                    "input": {"patchText": "*** Begin Patch\n+secret value\n*** End Patch"},
                    "error": format!(
                        "{DENY_RULE_PREFIX} Here are some of the relevant rules []"
                    )
                })
            )
        ),
    )
    .unwrap();

    let denials = parse_opencode_permission_denials(&path).unwrap();
    assert_eq!(denials.len(), 1);
    assert_eq!(denials[0].tool, "apply_patch");
    assert_eq!(denials[0].input_keys, vec!["patchText".to_string()]);
    // The patch body never reaches the report — keys only, never values.
    assert!(!format!("{denials:?}").contains("secret value"));
    assert!(!denials[0].reason.as_deref().unwrap().contains("Patch"));
}

#[test]
fn ordinary_tool_errors_are_not_permission_denials() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("opencode-events.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n{}\n",
            tool_use_error(
                "bash",
                json!({"status": "error", "input": {"command": "curl https://example.test"}, "error": "Could not resolve host: example.test"})
            ),
            tool_use_error(
                "edit",
                json!({"status": "error", "input": {"filePath": "/missing", "oldString": "a", "newString": "b"}, "error": "oldString not found"})
            ),
        ),
    )
    .unwrap();

    assert_eq!(
        parse_opencode_permission_denials(&path).unwrap(),
        Vec::new()
    );
}

#[test]
fn completed_tool_calls_and_non_tool_events_produce_no_denials() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("opencode-events.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n{}\n{}\n{}\n",
            json!({"type": "step_start", "timestamp": 1_000, "sessionID": "ses_1", "part": {"id": "p1", "type": "step-start"}}),
            tool_use_error("bash", json!({"status": "completed", "input": {"command": "ls"}, "output": "README.md"})),
            json!({"type": "text", "timestamp": 3_000, "sessionID": "ses_1", "part": {"id": "p3", "type": "text", "text": "done"}}),
            json!({"type": "step_finish", "timestamp": 4_000, "sessionID": "ses_1", "part": {"id": "p4", "type": "step-finish", "reason": "stop", "tokens": {"input": 1, "output": 1, "reasoning": 0, "cache": {"read": 0, "write": 0}}}})
        ),
    )
    .unwrap();

    assert_eq!(
        parse_opencode_permission_denials(&path).unwrap(),
        Vec::new()
    );
}

#[test]
fn malformed_or_denial_less_tool_parts_are_skipped() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("opencode-events.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n{}\n{}\n{}\n",
            json!({"type": "tool_use", "timestamp": 1, "sessionID": "ses_1"}),
            json!({"type": "tool_use", "timestamp": 2, "sessionID": "ses_1", "part": "not an object"}),
            json!({"type": "tool_use", "timestamp": 3, "sessionID": "ses_1", "part": {"id": "p1", "type": "tool"}}),
            tool_use_error("bash", json!({"status": "completed", "output": "ok"})),
        ),
    )
    .unwrap();

    assert_eq!(
        parse_opencode_permission_denials(&path).unwrap(),
        Vec::new()
    );
}

#[test]
fn a_missing_events_file_degrades_to_no_denials() {
    let dir = TempDir::new().unwrap();
    assert_eq!(
        parse_opencode_permission_denials(&dir.path().join("absent.jsonl")).unwrap(),
        Vec::new()
    );
}

#[test]
fn denials_accumulate_across_every_refused_call_in_the_stream() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("opencode-events.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n{}\n",
            tool_use_error(
                "bash",
                json!({"status": "error", "input": {"command": "pwd"}, "error": format!("{DENY_RULE_PREFIX} Here are some of the relevant rules []")})
            ),
            tool_use_error(
                "edit",
                json!({"status": "error", "input": {"filePath": "/x"}, "error": "eval guard: write to /x is outside the eval sandbox"})
            ),
        ),
    )
    .unwrap();

    assert_eq!(
        parse_opencode_permission_denials(&path).unwrap(),
        vec![
            denial("bash", DENY_RULE_PREFIX, &["command"]),
            denial(
                "edit",
                "eval guard: write to /x is outside the eval sandbox",
                &["filePath"]
            ),
        ]
    );
}
