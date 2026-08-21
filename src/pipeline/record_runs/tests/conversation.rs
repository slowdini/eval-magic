use super::*;
use crate::core::ConversationStatus;

#[test]
fn assembles_multi_turn_run_using_last_cumulative_codex_tokens_and_summed_duration() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "clarify",
            condition: "with_skill",
            final_message: None,
        }],
    );
    let conversation_path = iter
        .join("eval-clarify")
        .join("with_skill")
        .join("conversation.json");
    fs::write(
        &conversation_path,
        serde_json::to_string_pretty(&json!({
            "status": "completed",
            "delivered_followups": 1,
            "events": [
                {"type": "user_message", "ordinal": 0, "round": 1, "text": "Fix it."},
                {"type": "assistant_message", "ordinal": 1, "round": 1, "text": "Which timezone?"},
                {"type": "user_message", "ordinal": 2, "round": 2, "text": "US timezones."},
                {
                    "type": "tool_invocation",
                    "ordinal": 3,
                    "round": 2,
                    "name": "file_change",
                    "args": {"path": "src/date.rs"}
                },
                {
                    "type": "assistant_message",
                    "ordinal": 4,
                    "round": 2,
                    "text": "Updated the date handling."
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    for round in [1, 2] {
        let round_dir = paths[0].outputs_dir.join(format!("turn-{round}"));
        fs::create_dir_all(&round_dir).unwrap();
        write_codex_events(&round_dir, "unused");
    }
    let dispatch_path = iter.join("dispatch.json");
    let mut dispatch: Value =
        serde_json::from_str(&fs::read_to_string(&dispatch_path).unwrap()).unwrap();
    dispatch["tasks"][0]["conversation_path"] =
        json!(conversation_path.to_string_lossy().to_string());
    dispatch["tasks"][0]["turns"] =
        json!([{"prompt": "US timezones.", "deliver_when": "agent_asks"}]);
    fs::write(
        &dispatch_path,
        serde_json::to_string_pretty(&dispatch).unwrap(),
    )
    .unwrap();

    let result = record_runs(&iter, 1, Harness::resolve("codex").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 1);
    assert_eq!(result.missing_transcript, 0);

    let run = read_run(&iter, "clarify", "with_skill");
    assert_eq!(run.final_message, "Updated the date handling.");
    assert_eq!(
        serde_json::to_value(&run.tool_invocations).unwrap(),
        json!([{
            "name": "file_change",
            "args": {"path": "src/date.rs"},
            "ordinal": 3
        }])
    );
    assert_eq!(run.conversation.unwrap().delivered_followups, 1);

    let timing = read_timing_value(&iter, "clarify", "with_skill");
    assert_eq!(timing["total_tokens"], 40);
    assert_eq!(timing["duration_ms"], 60_000);
    assert_eq!(timing["source"], "transcript");
}

#[test]
fn assembles_multi_turn_run_by_summing_independent_claude_round_timing() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "clarify",
            condition: "with_skill",
            final_message: None,
        }],
    );
    let conversation_path = iter
        .join("eval-clarify")
        .join("with_skill")
        .join("conversation.json");
    fs::write(
        &conversation_path,
        serde_json::to_string_pretty(&json!({
            "status": "completed",
            "delivered_followups": 1,
            "events": [
                {"type": "user_message", "ordinal": 0, "round": 1, "text": "Fix it."},
                {"type": "assistant_message", "ordinal": 1, "round": 1, "text": "Which timezone?"},
                {"type": "user_message", "ordinal": 2, "round": 2, "text": "US timezones."},
                {"type": "assistant_message", "ordinal": 3, "round": 2, "text": "Done."}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    for round in [1, 2] {
        let round_dir = paths[0].outputs_dir.join(format!("turn-{round}"));
        fs::create_dir_all(&round_dir).unwrap();
        write_claude_events(&round_dir, "unused");
    }
    let dispatch_path = iter.join("dispatch.json");
    let mut dispatch: Value =
        serde_json::from_str(&fs::read_to_string(&dispatch_path).unwrap()).unwrap();
    dispatch["tasks"][0]["conversation_path"] =
        json!(conversation_path.to_string_lossy().to_string());
    fs::write(
        &dispatch_path,
        serde_json::to_string_pretty(&dispatch).unwrap(),
    )
    .unwrap();

    let result = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 1);
    assert_eq!(result.missing_transcript, 0);

    let timing = read_timing_value(&iter, "clarify", "with_skill");
    assert_eq!(timing["total_tokens"], 250);
    assert_eq!(timing["duration_ms"], 60_000);
    assert_eq!(timing["source"], "transcript");
}

#[test]
fn skips_multi_turn_run_when_conversation_shows_failed_prompt_read() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "clarify",
            condition: "with_skill",
            final_message: None,
        }],
    );
    let prompt_path = iter
        .join("eval-clarify")
        .join("with_skill")
        .join("dispatch-prompt.txt");
    fs::write(
        &prompt_path,
        format!("{PROMPT_SENTINEL}\n\nUser request:\nFix it."),
    )
    .unwrap();
    let conversation_path = iter
        .join("eval-clarify")
        .join("with_skill")
        .join("conversation.json");
    fs::write(
        &conversation_path,
        serde_json::to_string_pretty(&json!({
            "status": "completed",
            "delivered_followups": 0,
            "events": [
                {"type": "user_message", "ordinal": 0, "round": 1, "text": "Fix it."},
                {
                    "type": "tool_invocation",
                    "ordinal": 1,
                    "round": 1,
                    "name": "Read",
                    "args": {"file_path": prompt_path.to_string_lossy()},
                    "result": "<tool_use_error>File is outside the allowed working directory.</tool_use_error>"
                },
                {
                    "type": "assistant_message",
                    "ordinal": 2,
                    "round": 1,
                    "text": "I could not read the prompt file."
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let round_dir = paths[0].outputs_dir.join("turn-1");
    fs::create_dir_all(&round_dir).unwrap();
    write_claude_events(&round_dir, "unused");

    let dispatch_path = iter.join("dispatch.json");
    let mut dispatch: Value =
        serde_json::from_str(&fs::read_to_string(&dispatch_path).unwrap()).unwrap();
    dispatch["tasks"][0]["dispatch_prompt_path"] = json!(prompt_path.to_string_lossy().to_string());
    dispatch["tasks"][0]["conversation_path"] =
        json!(conversation_path.to_string_lossy().to_string());
    fs::write(
        &dispatch_path,
        serde_json::to_string_pretty(&dispatch).unwrap(),
    )
    .unwrap();

    let result = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();

    assert_eq!(result.skipped_prompt_unread, 1);
    assert_eq!(result.recorded, 0);
    assert!(!paths[0].run_record_path.exists());
    assert!(!paths[0].timing_path.exists());
}

#[test]
fn does_not_record_partial_timing_when_a_conversation_round_transcript_is_missing() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "clarify",
            condition: "with_skill",
            final_message: None,
        }],
    );
    let conversation_path = iter
        .join("eval-clarify")
        .join("with_skill")
        .join("conversation.json");
    fs::write(
        &conversation_path,
        serde_json::to_string_pretty(&json!({
            "status": "completed",
            "delivered_followups": 1,
            "events": [
                {"type": "user_message", "ordinal": 0, "round": 1, "text": "Fix it."},
                {"type": "assistant_message", "ordinal": 1, "round": 1, "text": "Which timezone?"},
                {"type": "user_message", "ordinal": 2, "round": 2, "text": "US timezones."},
                {"type": "assistant_message", "ordinal": 3, "round": 2, "text": "Done."}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let round_dir = paths[0].outputs_dir.join("turn-1");
    fs::create_dir_all(&round_dir).unwrap();
    write_codex_events(&round_dir, "unused");

    let dispatch_path = iter.join("dispatch.json");
    let mut dispatch: Value =
        serde_json::from_str(&fs::read_to_string(&dispatch_path).unwrap()).unwrap();
    dispatch["tasks"][0]["conversation_path"] =
        json!(conversation_path.to_string_lossy().to_string());
    fs::write(
        &dispatch_path,
        serde_json::to_string_pretty(&dispatch).unwrap(),
    )
    .unwrap();

    let result = record_runs(&iter, 1, Harness::resolve("codex").unwrap(), false).unwrap();

    assert_eq!(result.recorded, 1);
    assert_eq!(result.missing_transcript, 1);
    assert!(paths[0].run_record_path.exists());
    assert!(!paths[0].timing_path.exists());
}

/// A task whose completion artifact is missing is only "incomplete" if it was
/// meant to have rounds. That was keyed on `turns`, which a responder-driven
/// task does not declare — so without this it would be recorded from turn 1
/// alone, as though the conversation had never been interrupted.
#[test]
fn a_responder_task_without_its_completion_artifact_is_skipped_as_incomplete() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "clarify",
            condition: "with_skill",
            final_message: Some("Which cache?"),
        }],
    );
    let round_dir = paths[0].outputs_dir.join("turn-1");
    fs::create_dir_all(&round_dir).unwrap();
    write_claude_events(&round_dir, "Which cache?");

    let dispatch_path = iter.join("dispatch.json");
    let mut dispatch: Value =
        serde_json::from_str(&fs::read_to_string(&dispatch_path).unwrap()).unwrap();
    dispatch["tasks"][0]["responder"] = json!({ "type": "heuristic" });
    dispatch["tasks"][0]["conversation_path"] = json!(
        iter.join("eval-clarify")
            .join("with_skill")
            .join("conversation.json")
            .to_string_lossy()
    );
    fs::write(
        &dispatch_path,
        serde_json::to_string_pretty(&dispatch).unwrap(),
    )
    .unwrap();

    let result = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();

    assert_eq!(result.skipped_incomplete_conversation, 1);
    assert_eq!(result.recorded, 0);
    assert!(!paths[0].run_record_path.exists());
}

/// A task killed at its deadline still has rounds worth recording. Ingest
/// clones the whole conversation into `run.json` and validates it there, so the
/// run-record schema has to accept `timed_out` exactly as the conversation
/// schema does — otherwise a single hung task fails the whole ingest.
#[test]
fn records_a_run_whose_conversation_timed_out_in_a_later_round() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "clarify",
            condition: "with_skill",
            final_message: None,
        }],
    );
    let conversation_path = iter
        .join("eval-clarify")
        .join("with_skill")
        .join("conversation.json");
    fs::write(
        &conversation_path,
        serde_json::to_string_pretty(&json!({
            "status": "timed_out",
            "delivered_followups": 1,
            "timed_out_in_round": 2,
            "events": [
                {"type": "user_message", "ordinal": 0, "round": 1, "text": "Fix it."},
                {"type": "assistant_message", "ordinal": 1, "round": 1, "text": "Which timezone?"},
                {"type": "user_message", "ordinal": 2, "round": 2, "text": "US timezones."}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let round_dir = paths[0].outputs_dir.join("turn-1");
    fs::create_dir_all(&round_dir).unwrap();
    write_claude_events(&round_dir, "Which timezone?");

    let dispatch_path = iter.join("dispatch.json");
    let mut dispatch: Value =
        serde_json::from_str(&fs::read_to_string(&dispatch_path).unwrap()).unwrap();
    dispatch["tasks"][0]["responder"] = json!({ "type": "heuristic" });
    dispatch["tasks"][0]["conversation_path"] =
        json!(conversation_path.to_string_lossy().to_string());
    fs::write(
        &dispatch_path,
        serde_json::to_string_pretty(&dispatch).unwrap(),
    )
    .unwrap();

    let result = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();

    assert_eq!(result.recorded, 1);
    let run = read_run(&iter, "clarify", "with_skill");
    let conversation = run.conversation.unwrap();
    assert_eq!(conversation.status, ConversationStatus::TimedOut);
    assert_eq!(conversation.timed_out_in_round, Some(2));
}
