use super::*;
use serde_json::{Value, json};

#[test]
fn assertion_command_check_roundtrips_optional_fields_and_defaults_exit_zero() {
    let minimal: Assertion = serde_json::from_value(json!({
        "id": "all-consumers-correct",
        "type": "command_check",
        "command": "bun test ./holdout/test.ts"
    }))
    .unwrap();
    let Assertion::CommandCheck(minimal) = minimal else {
        panic!("expected command_check variant");
    };
    assert_eq!(minimal.expect_exit_code, 0);
    assert!(minimal.setup_files.is_none());
    assert!(minimal.env.is_none());
    assert!(minimal.matrix.is_none());
    assert!(minimal.expect_stdout.is_none());

    let full: Assertion = serde_json::from_value(json!({
        "id": "all-consumers-correct",
        "type": "command_check",
        "setup_files": ["holdout/test.ts"],
        "command": "bun test ./holdout/test.ts",
        "env": {
            "CI": "1",
            "TZ": "UTC"
        },
        "matrix": {
            "LOCALE": ["en_US", "de_DE"],
            "TZ": ["UTC", "America/Los_Angeles"]
        },
        "expect_exit_code": 2,
        "expect_stdout": "2 pass"
    }))
    .unwrap();
    let out = serde_json::to_value(full).unwrap();
    assert_eq!(out["type"], "command_check");
    assert_eq!(out["setup_files"], json!(["holdout/test.ts"]));
    assert_eq!(out["env"], json!({ "CI": "1", "TZ": "UTC" }));
    assert_eq!(
        out["matrix"],
        json!({
            "LOCALE": ["en_US", "de_DE"],
            "TZ": ["UTC", "America/Los_Angeles"]
        })
    );
    assert_eq!(out["expect_exit_code"], 2);
    assert_eq!(out["expect_stdout"], "2 pass");
}

#[test]
fn conditions_json_fixtures_round_trip_byte_identically() {
    for (name, fixture) in [
        (
            "claude-code",
            include_str!("../../../tests/fixtures/conditions/claude-code.json"),
        ),
        (
            "cline",
            include_str!("../../../tests/fixtures/conditions/cline.json"),
        ),
        (
            "codex",
            include_str!("../../../tests/fixtures/conditions/codex.json"),
        ),
        (
            "opencode",
            include_str!("../../../tests/fixtures/conditions/opencode.json"),
        ),
        (
            "no-harness",
            include_str!("../../../tests/fixtures/conditions/no-harness.json"),
        ),
    ] {
        let record: ConditionsRecord = serde_json::from_str(fixture)
            .unwrap_or_else(|e| panic!("fixture {name} no longer parses: {e}"));
        let mut out = serde_json::to_string_pretty(&record).unwrap();
        out.push('\n');
        assert_eq!(
            out, fixture,
            "fixture {name} did not round-trip byte-identically"
        );
    }
}

#[test]
fn conditions_json_with_unknown_harness_errors_naming_known_harnesses() {
    let err = serde_json::from_value::<ConditionsRecord>(json!({
        "mode": "new-skill",
        "conditions": [],
        "timestamp": "2026-06-08T00:00:00Z",
        "harness": "nonexistent"
    }))
    .unwrap_err()
    .to_string();
    assert!(err.contains("unknown harness 'nonexistent'"), "{err}");
    for name in ["claude-code", "cline", "codex", "opencode"] {
        assert!(err.contains(name), "error must name {name}: {err}");
    }
}

#[test]
fn command_check_grader_roundtrips_snake_case() {
    let value = serde_json::to_value(Grader::CommandCheck).unwrap();
    assert_eq!(value, Value::String("command_check".into()));
    let back: Grader = serde_json::from_value(value).unwrap();
    assert_eq!(back, Grader::CommandCheck);
}

#[test]
fn assertion_diff_scope_roundtrips_thresholds() {
    let parsed: Assertion = serde_json::from_value(json!({
        "id": "minimal-fix",
        "type": "diff_scope",
        "max_files_touched": 1,
        "max_lines_changed": 8
    }))
    .unwrap();
    let Assertion::DiffScope(diff_scope) = parsed else {
        panic!("expected diff_scope variant");
    };
    assert_eq!(diff_scope.id, "minimal-fix");
    assert_eq!(diff_scope.max_files_touched, Some(1));
    assert_eq!(diff_scope.max_lines_changed, Some(8));

    let out = serde_json::to_value(Assertion::DiffScope(diff_scope)).unwrap();
    assert_eq!(out["type"], "diff_scope");
    assert_eq!(out["max_files_touched"], 1);
    assert_eq!(out["max_lines_changed"], 8);
}

#[test]
fn diff_scope_grader_roundtrips_snake_case() {
    let value = serde_json::to_value(Grader::DiffScope).unwrap();
    assert_eq!(value, Value::String("diff_scope".into()));
    let back: Grader = serde_json::from_value(value).unwrap();
    assert_eq!(back, Grader::DiffScope);
}

#[test]
fn run_record_roundtrips_a_stopped_multi_turn_conversation() {
    let record: RunRecord = serde_json::from_value(json!({
        "eval_id": "timezone",
        "condition": "with_skill",
        "skill_path": "/skills/investigating-bugs/SKILL.md",
        "prompt": "The due date is wrong. Fix it.",
        "files": [],
        "final_message": "Which users are affected?",
        "tool_invocations": [],
        "total_tokens": null,
        "duration_ms": null,
        "conversation": {
            "status": "stopped",
            "delivered_followups": 0,
            "stop_reason": "agent_response_mismatch",
            "stopped_before_followup": 1,
            "events": [
                {
                    "type": "user_message",
                    "ordinal": 0,
                    "round": 1,
                    "text": "The due date is wrong. Fix it."
                },
                {
                    "type": "assistant_message",
                    "ordinal": 1,
                    "round": 1,
                    "text": "Which users are affected?"
                }
            ]
        }
    }))
    .unwrap();

    let conversation = record.conversation.as_ref().unwrap();
    assert_eq!(conversation.status, ConversationStatus::Stopped);
    assert_eq!(
        conversation.stop_reason,
        Some(ConversationStopReason::AgentResponseMismatch)
    );
    assert_eq!(conversation.events.len(), 2);

    let out = serde_json::to_value(record).unwrap();
    assert_eq!(out["conversation"]["stopped_before_followup"], 1);
    assert_eq!(
        out["conversation"]["events"][1]["type"],
        "assistant_message"
    );
}

/// A responder-driven record has to satisfy three contracts at once: the Rust
/// types, `conversation.schema.json` (which the driver validates against before
/// writing), and `run-record.schema.json` (which ingest validates against
/// afterwards). Checking one alone lets the other two drift.
#[test]
fn a_responder_record_satisfies_both_schemas_and_roundtrips() {
    use crate::validation::{SchemaName, validate_against_schema};

    let conversation = json!({
        "status": "stopped",
        "delivered_followups": 1,
        "stop_reason": "responder_cannot_answer",
        "stopped_before_followup": 2,
        "events": [
            { "type": "user_message", "ordinal": 0, "round": 1, "text": "Add caching." },
            { "type": "assistant_message", "ordinal": 1, "round": 1, "text": "Which cache?\n\n- LRU (Recommended)\n- Redis\n" },
            {
                "type": "user_message",
                "ordinal": 2,
                "round": 2,
                "text": "LRU",
                "origin": {
                    "responder": "heuristic",
                    "answers": [{
                        "question": "Which cache?",
                        "options": ["LRU (Recommended)", "Redis"],
                        "rule": "recommended_option",
                        "chosen": ["LRU"]
                    }]
                }
            },
            { "type": "assistant_message", "ordinal": 3, "round": 2, "text": "What TTL suits you?" }
        ]
    });

    let parsed: ConversationRecord =
        validate_against_schema(SchemaName::Conversation, &conversation, "conversation.json")
            .unwrap();
    assert_eq!(
        parsed.stop_reason,
        Some(ConversationStopReason::ResponderCannotAnswer)
    );
    let ConversationEvent::UserMessage { origin, .. } = &parsed.events[2] else {
        panic!("event 2 is the synthesized turn");
    };
    let origin = origin
        .as_ref()
        .expect("a synthesized turn names its origin");
    assert_eq!(origin.responder, ResponderKind::Heuristic);
    assert_eq!(origin.answers[0].rule, ResponderRule::RecommendedOption);

    // The seeded prompt is authored, not derived, so it carries no origin at
    // all — the field's absence is what distinguishes the two.
    let ConversationEvent::UserMessage { origin, .. } = &parsed.events[0] else {
        panic!("event 0 is the eval prompt");
    };
    assert!(origin.is_none());

    let record = json!({
        "eval_id": "add-caching",
        "condition": "with_skill",
        "skill_path": null,
        "prompt": "Add caching.",
        "files": [],
        "final_message": "What TTL suits you?",
        "tool_invocations": [],
        "total_tokens": null,
        "duration_ms": null,
        "conversation": conversation
    });
    let record: RunRecord =
        validate_against_schema(SchemaName::RunRecord, &record, "run.json").unwrap();

    assert_eq!(
        serde_json::to_value(&record).unwrap()["conversation"],
        serde_json::to_value(record.conversation.clone().unwrap()).unwrap()
    );
}

/// A conversation that outran its deadline is written by the driver and read
/// back by ingest, so the run-record schema has to accept the same shape
/// `conversation.schema.json` does — including a round-1 timeout, whose only
/// event is the seeded prompt.
#[test]
fn a_timed_out_conversation_satisfies_the_run_record_schema() {
    use crate::validation::{SchemaName, validate_against_schema};

    let conversation = json!({
        "status": "timed_out",
        "delivered_followups": 0,
        "timed_out_in_round": 1,
        "events": [
            { "type": "user_message", "ordinal": 0, "round": 1, "text": "Add caching." }
        ]
    });
    let _: ConversationRecord =
        validate_against_schema(SchemaName::Conversation, &conversation, "conversation.json")
            .unwrap();

    let record = json!({
        "eval_id": "add-caching",
        "condition": "with_skill",
        "skill_path": null,
        "prompt": "Add caching.",
        "files": [],
        "final_message": "",
        "tool_invocations": [],
        "total_tokens": null,
        "duration_ms": null,
        "conversation": conversation
    });

    let record: RunRecord =
        validate_against_schema(SchemaName::RunRecord, &record, "run.json").unwrap();
    assert_eq!(
        record.conversation.unwrap().status,
        ConversationStatus::TimedOut
    );
}
