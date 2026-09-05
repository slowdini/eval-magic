use super::*;
use crate::core::ConversationStatus;

#[test]
fn assembles_multi_turn_run_using_codex_tokens_and_runner_duration() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "clarify",
            condition: "with_skill",
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
            "duration_ms": 123,
            "events": [
                {"type": "user_message", "ordinal": 0, "round": 1, "text": "Fix it."},
                {"type": "user_message", "ordinal": 1, "round": 2, "text": "US timezones."}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    for (round, final_text) in [(1, "Which timezone?"), (2, "Updated the date handling.")] {
        let round_dir = paths[0].outputs_dir.join(format!("turn-{round}"));
        fs::create_dir_all(&round_dir).unwrap();
        write_codex_events(&round_dir, final_text);
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
        json!([
            {
                "name": "command_execution",
                "args": {"command": "bun test"},
                "ordinal": 1,
                "result": "ok"
            },
            {
                "name": "command_execution",
                "args": {"command": "bun test"},
                "ordinal": 4,
                "result": "ok"
            }
        ])
    );
    let recorded_conversation = serde_json::to_value(run.conversation.unwrap()).unwrap();
    assert!(
        recorded_conversation.get("duration_ms").is_none(),
        "run.json keeps timing in timing.json: {recorded_conversation}"
    );
    assert_eq!(
        recorded_conversation["events"],
        json!([
            {"type": "user_message", "ordinal": 0, "round": 1, "text": "Fix it."},
            {"type": "tool_invocation", "ordinal": 1, "round": 1, "name": "command_execution", "args": {"command": "bun test"}, "result": "ok"},
            {"type": "assistant_message", "ordinal": 2, "round": 1, "text": "Which timezone?"},
            {"type": "user_message", "ordinal": 3, "round": 2, "text": "US timezones."},
            {"type": "tool_invocation", "ordinal": 4, "round": 2, "name": "command_execution", "args": {"command": "bun test"}, "result": "ok"},
            {"type": "assistant_message", "ordinal": 5, "round": 2, "text": "Updated the date handling."}
        ])
    );

    let timing = read_timing_value(&iter, "clarify", "with_skill");
    assert_eq!(timing["total_tokens"], 40);
    assert_eq!(timing["duration_ms"], 123);
    assert_eq!(timing["token_source"], "transcript");
    assert_eq!(timing["duration_source"], "runner");
}

#[test]
fn one_shot_task_without_its_completion_artifact_is_skipped_as_incomplete() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "one-shot",
            condition: "with_skill",
        }],
    );
    let round_dir = paths[0].outputs_dir.join("turn-1");
    fs::create_dir_all(&round_dir).unwrap();
    write_claude_events(&round_dir, "Done.");

    let dispatch_path = iter.join("dispatch.json");
    let mut dispatch: Value =
        serde_json::from_str(&fs::read_to_string(&dispatch_path).unwrap()).unwrap();
    dispatch["tasks"][0]["conversation_path"] = json!(
        iter.join("eval-one-shot")
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
    let warning = result
        .incomplete_conversation_warning()
        .expect("missing completion artifact is reported");
    assert!(warning.contains("1 task skipped"), "{warning}");
    assert!(!warning.contains("multi-turn"), "{warning}");
    assert!(!paths[0].run_record_path.exists());
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
    assert_eq!(timing["token_source"], "transcript");
    assert_eq!(timing["duration_source"], "transcript");
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
                {"type": "user_message", "ordinal": 0, "round": 1, "text": "Fix it."}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let round_dir = paths[0].outputs_dir.join("turn-1");
    fs::create_dir_all(&round_dir).unwrap();
    write_claude_events_prompt_read(
        &round_dir,
        &prompt_path.to_string_lossy(),
        "<tool_use_error>File is outside the allowed working directory.</tool_use_error>",
        "I could not read the prompt file.",
    );

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
fn records_runner_duration_without_partial_tokens_when_a_round_transcript_is_missing() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "clarify",
            condition: "with_skill",
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
            "duration_ms": 321,
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
    let timing = read_timing_value(&iter, "clarify", "with_skill");
    assert!(timing.get("total_tokens").is_none());
    assert!(timing.get("token_source").is_none());
    assert_eq!(timing["duration_ms"], 321);
    assert_eq!(timing["duration_source"], "runner");
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
        }],
    );
    let round_dir = paths[0].outputs_dir.join("turn-1");
    fs::create_dir_all(&round_dir).unwrap();
    write_claude_events(&round_dir, "Which cache?");

    let dispatch_path = iter.join("dispatch.json");
    let mut dispatch: Value =
        serde_json::from_str(&fs::read_to_string(&dispatch_path).unwrap()).unwrap();
    dispatch["tasks"][0]["responder"] = json!({ "type": "llm" });
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
            "duration_ms": 456,
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
    dispatch["tasks"][0]["responder"] = json!({ "type": "llm" });
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
    assert_eq!(conversation.duration_ms, None);
    let timing = read_timing_value(&iter, "clarify", "with_skill");
    assert!(timing.get("total_tokens").is_none(), "{timing}");
    assert_eq!(timing["duration_ms"], 456);
    assert_eq!(timing["duration_source"], "runner");
}

/// The session mode each round ran in survives ingest, so a judge can tell the
/// planning rounds from the implementation.
#[test]
fn plan_mode_rounds_keep_their_mode_through_ingest() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "plan-first",
            condition: "with_skill",
        }],
    );
    let conversation_path = iter
        .join("eval-plan-first")
        .join("with_skill")
        .join("conversation.json");
    fs::write(
        &conversation_path,
        serde_json::to_string_pretty(&json!({
            "status": "completed",
            "delivered_followups": 1,
            "events": [
                {"type": "user_message", "ordinal": 0, "round": 1, "text": "Add caching.", "mode": "plan"},
                {"type": "user_message", "ordinal": 1, "round": 2,
                 "text": "The plan is approved. Implement it now.",
                 "origin": {"runner": "plan_approval"}, "mode": "act"}
            ],
            "plan": {"presented_in_round": 1, "approved_in_round": 2, "signal": "plan_file"}
        }))
        .unwrap(),
    )
    .unwrap();
    for (round, final_text) in [(1, "Here is the plan."), (2, "Implemented.")] {
        let round_dir = paths[0].outputs_dir.join(format!("turn-{round}"));
        fs::create_dir_all(&round_dir).unwrap();
        write_codex_events(&round_dir, final_text);
    }
    let dispatch_path = iter.join("dispatch.json");
    let mut dispatch: Value =
        serde_json::from_str(&fs::read_to_string(&dispatch_path).unwrap()).unwrap();
    dispatch["tasks"][0]["conversation_path"] =
        json!(conversation_path.to_string_lossy().to_string());
    dispatch["tasks"][0]["plan_mode"] = json!(true);
    fs::write(
        &dispatch_path,
        serde_json::to_string_pretty(&dispatch).unwrap(),
    )
    .unwrap();

    let result = record_runs(&iter, 1, Harness::resolve("codex").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 1);

    let run = read_run(&iter, "plan-first", "with_skill");
    let conversation = serde_json::to_value(run.conversation.unwrap()).unwrap();
    let user_turns: Vec<&Value> = conversation["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "user_message")
        .collect();
    assert_eq!(user_turns[0]["mode"], "plan");
    assert_eq!(user_turns[1]["mode"], "act");
    assert_eq!(user_turns[1]["origin"], json!({"runner": "plan_approval"}));
    assert_eq!(conversation["plan"]["signal"], "plan_file");
}
