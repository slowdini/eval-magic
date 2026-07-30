//! `record-runs` permission-denial collection: the iteration artifact, guard
//! attribution, harnesses that cannot detect a refusal, and accumulation across
//! scripted rounds.

use super::*;

/// A `claude -p` events fixture whose terminal `result` event reports one
/// refused tool call, with `reason` as the refusal text the agent saw.
fn write_claude_events_with_denial(outputs_dir: &Path, reason: &str) {
    let lines = vec![
        json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "bun run repro.ts"}}]}}),
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": reason, "is_error": true}]}}),
        json!({"type": "result", "subtype": "success", "is_error": false, "result": "Verified by reasoning.", "duration_ms": 30_000, "usage": {"input_tokens": 100, "output_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 5},
            "permission_denials": [{"tool_name": "Bash", "tool_use_id": "toolu_1", "tool_input": {"command": "bun run repro.ts"}}]}),
    ];
    fs::write(outputs_dir.join("claude-events.jsonl"), jsonl(&lines)).unwrap();
}

fn write_codex_stderr_with_denial(outputs_dir: &Path, reason: &str) {
    write_codex_events(outputs_dir, "Verified by reasoning.");
    fs::write(
        outputs_dir.join("codex-stderr.log"),
        format!(
            "2026-07-30T06:09:01Z ERROR codex_core::tools::router: \
             error=Command blocked by PreToolUse hook: {reason}. Command: pwd\n"
        ),
    )
    .unwrap();
}

fn read_permission_denials(iteration_dir: &Path) -> Value {
    let raw = fs::read_to_string(iteration_dir.join("permission-denials.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn collects_codex_permission_denials_from_the_sibling_stderr_capture() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "tz-bug",
            condition: "with_skill",
            final_message: Some("Verified by reasoning."),
        }],
    );
    write_codex_stderr_with_denial(&paths[0].outputs_dir, "permission-denial probe");

    let result = record_runs(&iter, 2, Harness::resolve("codex").unwrap(), false).unwrap();

    assert_eq!(result.recorded, 1);
    assert_eq!(result.permission_denials, 1);
    assert_eq!(result.permission_denial_tasks, 1);
    let report = read_permission_denials(&iter);
    assert_eq!(report["iteration"], 2);
    assert_eq!(report["total_denials"], 1);
    assert_eq!(report["tasks"][0]["denials"][0]["tool"], "Bash");
    assert_eq!(
        report["tasks"][0]["denials"][0]["reason"],
        "permission-denial probe"
    );
    assert_eq!(
        report["tasks"][0]["denials"][0]["input_keys"],
        json!(["command"])
    );
}

#[test]
fn collects_permission_denials_from_each_tasks_transcript() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[
            FixtureTask {
                eval_id: "tz-bug",
                condition: "with_skill",
                final_message: Some("Verified by reasoning."),
            },
            FixtureTask {
                eval_id: "tz-bug",
                condition: "without_skill",
                final_message: Some("Fixed it."),
            },
        ],
    );
    write_claude_events_with_denial(&paths[0].outputs_dir, "This command requires approval");
    write_claude_events(&paths[1].outputs_dir, "unused");

    let result = record_runs(&iter, 3, Harness::resolve("claude-code").unwrap(), false).unwrap();

    // Grading is untouched: both arms still record normally.
    assert_eq!(result.recorded, 2);
    assert_eq!(result.permission_denials, 1);
    assert_eq!(result.permission_denial_tasks, 1);

    let report = read_permission_denials(&iter);
    assert_eq!(report["iteration"], 3);
    assert_eq!(report["total_denials"], 1);
    let tasks = report["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["eval_id"], "tz-bug");
    assert_eq!(tasks[0]["condition"], "with_skill");
    assert_eq!(tasks[0]["denial_count"], 1);
    assert_eq!(tasks[0]["guard_attributed_count"], 0);
    assert_eq!(tasks[0]["denials"][0]["tool"], "Bash");
    assert_eq!(
        tasks[0]["denials"][0]["reason"],
        "This command requires approval"
    );
    assert_eq!(tasks[0]["denials"][0]["input_keys"], json!(["command"]));
}

#[test]
fn guard_blocked_calls_are_attributed_and_not_counted_as_harness_refusals() {
    // The guard's own block is recorded for completeness but excluded from the
    // count `aggregate` warns on — guard-denials.json already reports it.
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "tz-bug",
            condition: "with_skill",
            final_message: Some("Done."),
        }],
    );
    write_claude_events_with_denial(
        &paths[0].outputs_dir,
        "eval guard: blocked Bash (writes outside) — runs outside the eval sandbox",
    );

    let result = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();
    assert_eq!(result.permission_denials, 0);
    assert_eq!(result.permission_denial_tasks, 0);

    let report = read_permission_denials(&iter);
    assert_eq!(report["total_denials"], 1);
    assert_eq!(report["tasks"][0]["guard_attributed_count"], 1);
    assert_eq!(report["tasks"][0]["denials"][0]["guard_attributed"], true);
}

#[test]
fn codex_guard_blocks_are_attributed_and_not_counted_as_harness_refusals() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "tz-bug",
            condition: "with_skill",
            final_message: Some("Done."),
        }],
    );
    write_codex_stderr_with_denial(
        &paths[0].outputs_dir,
        "eval guard: blocked Bash (writes outside)",
    );

    let result = record_runs(&iter, 1, Harness::resolve("codex").unwrap(), false).unwrap();
    assert_eq!(result.permission_denials, 0);
    assert_eq!(result.permission_denial_tasks, 0);

    let report = read_permission_denials(&iter);
    assert_eq!(report["total_denials"], 1);
    assert_eq!(report["tasks"][0]["guard_attributed_count"], 1);
    assert_eq!(report["tasks"][0]["denials"][0]["guard_attributed"], true);
}

#[test]
fn no_permission_denials_artifact_for_a_harness_that_cannot_detect_them() {
    // Absence of the file means "not detected", never "zero denials" — so it
    // is not written at all rather than written empty.
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "crash",
            condition: "with_skill",
            final_message: Some("Fixed it."),
        }],
    );
    fs::write(
        paths[0].outputs_dir.join("opencode-events.jsonl"),
        concat!(
            r#"{"type":"text","timestamp":1000,"sessionID":"ses_1","part":{"id":"p1","type":"text","text":"Fixed it."}}"#,
            "\n"
        ),
    )
    .unwrap();

    record_runs(&iter, 1, Harness::resolve("opencode").unwrap(), false).unwrap();
    assert!(!iter.join("permission-denials.json").exists());
}

#[test]
fn permission_denials_survive_a_dispatch_skipped_as_prompt_unread() {
    // A refused prompt read is exactly the case worth reporting, so denials
    // are collected before any skip decision drops the task.
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "e1",
            condition: "with_skill",
            final_message: Some("I could not read the prompt file."),
        }],
    );
    let prompt_path = iter
        .join("eval-e1")
        .join("with_skill")
        .join("dispatch-prompt.txt");
    fs::write(
        &prompt_path,
        format!("{PROMPT_SENTINEL}\n\nUser request:\ndo it"),
    )
    .unwrap();
    let lines = vec![
        json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"file_path": prompt_path.to_string_lossy()}}]}}),
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "This tool requires approval", "is_error": true}]}}),
        json!({"type": "result", "subtype": "success", "is_error": false, "result": "I could not read the prompt file.", "duration_ms": 10,
            "permission_denials": [{"tool_name": "Read", "tool_use_id": "toolu_1", "tool_input": {"file_path": prompt_path.to_string_lossy()}}]}),
    ];
    fs::write(
        paths[0].outputs_dir.join("claude-events.jsonl"),
        jsonl(&lines),
    )
    .unwrap();

    let result = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();
    assert_eq!(result.skipped_prompt_unread, 1);
    assert_eq!(result.recorded, 0);
    assert_eq!(result.permission_denials, 1);
    assert_eq!(read_permission_denials(&iter)["total_denials"], 1);
}

#[test]
fn permission_denial_warning_names_the_artifact_and_only_fires_on_harness_refusals() {
    let mut result = RecordRunsResult::default();
    assert_eq!(result.permission_denial_warning(), None);

    result.permission_denials = 3;
    result.permission_denial_tasks = 2;
    let warning = result.permission_denial_warning().unwrap();
    assert!(warning.contains('3'), "{warning}");
    assert!(warning.contains("2 task"), "{warning}");
    assert!(warning.contains("permission-denials.json"), "{warning}");
}

#[test]
fn permission_denials_accumulate_across_every_scripted_round() {
    // Each round is its own CLI invocation with its own terminal result event, so
    // a refusal in any round has to be picked up — not just the last one's.
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
        write_claude_events_with_denial(&round_dir, "This command requires approval");
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
    assert_eq!(result.permission_denials, 2);
    assert_eq!(result.permission_denial_tasks, 1);

    let report = read_permission_denials(&iter);
    assert_eq!(report["total_denials"], 2);
    assert_eq!(report["tasks"][0]["denial_count"], 2);
}

#[test]
fn codex_permission_denials_accumulate_across_every_scripted_round() {
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
        write_codex_stderr_with_denial(&round_dir, "This command requires approval");
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

    let result = record_runs(&iter, 1, Harness::resolve("codex").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 1);
    assert_eq!(result.permission_denials, 2);
    assert_eq!(result.permission_denial_tasks, 1);

    let report = read_permission_denials(&iter);
    assert_eq!(report["total_denials"], 2);
    assert_eq!(report["tasks"][0]["denial_count"], 2);
}
