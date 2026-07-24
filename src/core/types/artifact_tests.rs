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
    assert!(minimal.expect_stdout.is_none());

    let full: Assertion = serde_json::from_value(json!({
        "id": "all-consumers-correct",
        "type": "command_check",
        "setup_files": ["holdout/test.ts"],
        "command": "bun test ./holdout/test.ts",
        "expect_exit_code": 2,
        "expect_stdout": "2 pass"
    }))
    .unwrap();
    let out = serde_json::to_value(full).unwrap();
    assert_eq!(out["type"], "command_check");
    assert_eq!(out["setup_files"], json!(["holdout/test.ts"]));
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
    for name in ["claude-code", "codex", "opencode"] {
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
