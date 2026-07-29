//! `RecordRunsResult::transcript_warning` copy: when it fires and which
//! harness-native events file it points the operator at.

use super::*;

#[test]
fn no_transcript_warning_when_all_transcripts_matched() {
    let result = RecordRunsResult {
        recorded: 4,
        missing_transcript: 0,
        ..Default::default()
    };
    assert!(
        result
            .transcript_warning(Harness::resolve("claude-code").unwrap())
            .is_none()
    );
}

#[test]
fn claude_code_warning_fires_on_partial_miss() {
    let result = RecordRunsResult {
        recorded: 4,
        missing_transcript: 1,
        ..Default::default()
    };
    let warning = result
        .transcript_warning(Harness::resolve("claude-code").unwrap())
        .unwrap();
    assert!(warning.contains('1'), "names the count: {warning}");
}

#[test]
fn codex_warning_points_at_events_file() {
    let result = RecordRunsResult {
        recorded: 2,
        missing_transcript: 2,
        ..Default::default()
    };
    let warning = result
        .transcript_warning(Harness::resolve("codex").unwrap())
        .unwrap();
    assert!(
        warning.contains("codex-events.jsonl"),
        "names the Codex source: {warning}"
    );
    assert!(
        warning.contains("under task outputs"),
        "covers one-shot and per-turn transcript locations: {warning}"
    );
    assert!(
        !warning.contains("agent_description"),
        "Codex doesn't use agent_description: {warning}"
    );
}

#[test]
fn claude_warning_points_at_events_file() {
    let result = RecordRunsResult {
        recorded: 2,
        missing_transcript: 2,
        ..Default::default()
    };
    let warning = result
        .transcript_warning(Harness::resolve("claude-code").unwrap())
        .unwrap();
    assert!(
        warning.contains("claude-events.jsonl"),
        "names the Claude CLI events source: {warning}"
    );
    assert!(
        !warning.contains("agent_description"),
        "CLI dispatch doesn't use agent_description: {warning}"
    );
}
