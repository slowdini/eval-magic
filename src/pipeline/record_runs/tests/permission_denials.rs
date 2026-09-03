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
    write_transcript_file(outputs_dir, "claude-events.jsonl", jsonl(&lines));
}

fn write_codex_stderr_with_denial(outputs_dir: &Path, reason: &str) {
    write_codex_events(outputs_dir, "Verified by reasoning.");
    write_transcript_file(
        outputs_dir,
        "codex-stderr.log",
        format!(
            "2026-07-30T06:09:01Z ERROR codex_core::tools::router: \
             error=Command blocked by PreToolUse hook: {reason}. Command: pwd\n"
        ),
    );
}

/// OpenCode records a refusal as a `tool_use` event whose `state.error` carries
/// the refusal explanation. `error` is the full `state.error` string; `tool`
/// and `input` shape the refused call (input *keys* only reach the report).
fn write_opencode_events_with_denial(
    outputs_dir: &Path,
    error: &str,
    tool: &str,
    input: serde_json::Value,
) {
    let lines = vec![
        json!({"type": "text", "timestamp": 1_000, "sessionID": "ses_1", "part": {"id": "p1", "type": "text", "text": "Verified by reasoning."}}),
        json!({"type": "step_finish", "timestamp": 2_000, "sessionID": "ses_1", "part": {"id": "p2", "type": "step-finish", "reason": "stop", "tokens": {"input": 1, "output": 1, "reasoning": 0, "cache": {"read": 0, "write": 0}}}}),
        json!({
            "type": "tool_use", "timestamp": 1_500, "sessionID": "ses_1",
            "part": {
                "id": "pt_1", "sessionID": "ses_1", "messageID": "msg_1", "type": "tool", "callID": "c_1",
                "tool": tool,
                "state": {"status": "error", "input": input, "error": error, "time": {"start": 1_400, "end": 1_500}}
            }
        }),
    ];
    write_transcript_file(outputs_dir, "opencode-events.jsonl", jsonl(&lines));
}

/// The fixed prefix OpenCode's `PermissionDeniedError` emits before its ruleset
/// JSON — matches the parser constant so this test breaks if the marker drifts.
const OPENCODE_DENY_RULE_PREFIX: &str =
    "The user has specified a rule which prevents you from using this specific tool call.";

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
            },
            FixtureTask {
                eval_id: "tz-bug",
                condition: "without_skill",
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
fn opencode_writes_a_zero_denial_report_when_nothing_was_refused() {
    // OpenCode now detects denials, so "detected and nothing refused" writes a
    // zero-denial report (the file's presence means "detected"); absence of the
    // file would now mean "not detected", never "zero denials".
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "crash",
            condition: "with_skill",
        }],
    );
    write_opencode_events(&paths[0].outputs_dir, "Fixed it.");

    record_runs(&iter, 1, Harness::resolve("opencode").unwrap(), false).unwrap();
    let report = read_permission_denials(&iter);
    assert_eq!(report["iteration"], 1);
    assert_eq!(report["total_denials"], 0);
    assert_eq!(report["tasks"], serde_json::json!([]));
}

#[test]
fn collects_opencode_permission_denials_from_the_event_stream() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "tz-bug",
            condition: "with_skill",
        }],
    );
    write_opencode_events_with_denial(
        &paths[0].outputs_dir,
        &format!(
            "{OPENCODE_DENY_RULE_PREFIX} Here are some of the relevant rules \
            [{{\"permission\":\"bash\",\"pattern\":\"pwd\",\"action\":\"deny\"}}]"
        ),
        "bash",
        json!({"command": "pwd"}),
    );

    let result = record_runs(&iter, 2, Harness::resolve("opencode").unwrap(), false).unwrap();

    assert_eq!(result.recorded, 1);
    assert_eq!(result.permission_denials, 1);
    assert_eq!(result.permission_denial_tasks, 1);
    let report = read_permission_denials(&iter);
    assert_eq!(report["iteration"], 2);
    assert_eq!(report["total_denials"], 1);
    assert_eq!(report["tasks"][0]["denials"][0]["tool"], "bash");
    // The ruleset is stripped to the fixed refusal prefix.
    assert_eq!(
        report["tasks"][0]["denials"][0]["reason"],
        OPENCODE_DENY_RULE_PREFIX
    );
    assert_eq!(
        report["tasks"][0]["denials"][0]["input_keys"],
        json!(["command"])
    );
    assert_eq!(report["tasks"][0]["guard_attributed_count"], 0);
}

#[test]
fn opencode_guard_blocks_are_attributed_and_not_counted_as_harness_refusals() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "tz-bug",
            condition: "with_skill",
        }],
    );
    write_opencode_events_with_denial(
        &paths[0].outputs_dir,
        "eval guard: write to /tmp/x is outside the eval sandbox",
        "edit",
        json!({"filePath": "/tmp/x", "oldString": "a", "newString": "b"}),
    );

    let result = record_runs(&iter, 1, Harness::resolve("opencode").unwrap(), false).unwrap();
    assert_eq!(result.permission_denials, 0);
    assert_eq!(result.permission_denial_tasks, 0);

    let report = read_permission_denials(&iter);
    assert_eq!(report["total_denials"], 1);
    assert_eq!(report["tasks"][0]["guard_attributed_count"], 1);
    assert_eq!(report["tasks"][0]["denials"][0]["guard_attributed"], true);
    // input *keys* only — the file body never reaches the report.
    assert_eq!(
        report["tasks"][0]["denials"][0]["input_keys"],
        json!(["filePath", "newString", "oldString"])
    );
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
    write_transcript_file(&paths[0].outputs_dir, "claude-events.jsonl", jsonl(&lines));

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

#[test]
fn opencode_permission_denials_accumulate_across_every_scripted_round() {
    // Each scripted round is its own `opencode run` with its own events file, so
    // a refusal in any round must be picked up — not just the last one's.
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
        write_opencode_events_with_denial(
            &round_dir,
            &format!("{OPENCODE_DENY_RULE_PREFIX} Here are some of the relevant rules []"),
            "bash",
            json!({"command": "pwd"}),
        );
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

    let result = record_runs(&iter, 1, Harness::resolve("opencode").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 1);
    assert_eq!(result.permission_denials, 2);
    assert_eq!(result.permission_denial_tasks, 1);

    let report = read_permission_denials(&iter);
    assert_eq!(report["total_denials"], 2);
    assert_eq!(report["tasks"][0]["denial_count"], 2);
}

/// A claude events fixture whose refused call is a write, the way plan mode
/// refuses one: `Cannot write to <path> while in plan mode.`
fn write_claude_events_with_write_denial(outputs_dir: &Path, reason: &str) {
    let lines = vec![
        json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Edit", "input": {"file_path": "/env/pricing.py", "old_string": "a", "new_string": "b"}}]}}),
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": reason, "is_error": true}]}}),
        json!({"type": "result", "subtype": "success", "is_error": false, "result": "Here is the plan.", "duration_ms": 30_000, "usage": {"input_tokens": 100, "output_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 5},
            "permission_denials": [{"tool_name": "Edit", "tool_use_id": "toolu_1", "tool_input": {"file_path": "/env/pricing.py", "old_string": "a", "new_string": "b"}}]}),
    ];
    write_transcript_file(outputs_dir, "claude-events.jsonl", jsonl(&lines));
}

/// An agent that tries to edit while planning is refused by the mode itself, so
/// the refusal is recorded but not counted against the run. The same refusal in
/// the act round is the report's own, and warned about.
#[test]
fn a_write_refused_in_a_planning_round_is_attributed_to_plan_mode() {
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
                {"type": "user_message", "ordinal": 0, "round": 1, "text": "Fix total().", "mode": "plan"},
                {"type": "user_message", "ordinal": 1, "round": 2,
                 "text": "The plan is approved. Implement it now.",
                 "origin": {"runner": "plan_approval"}, "mode": "act"}
            ],
            "plan": {"presented_in_round": 1, "approved_in_round": 2, "signal": "plan_file"}
        }))
        .unwrap(),
    )
    .unwrap();
    for round in [1, 2] {
        let round_dir = paths[0].outputs_dir.join(format!("turn-{round}"));
        fs::create_dir_all(&round_dir).unwrap();
        write_claude_events_with_write_denial(
            &round_dir,
            "Cannot write to /env/pricing.py while in plan mode.",
        );
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

    let result = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 1);
    // Two refusals recorded, one of them the mode working as designed, so only
    // the act-round refusal is counted and warned about.
    assert_eq!(result.permission_denials, 1);
    assert_eq!(result.permission_denial_tasks, 1);

    let report = read_permission_denials(&iter);
    assert_eq!(report["total_denials"], 2);
    let task = &report["tasks"][0];
    assert_eq!(task["denial_count"], 2);
    assert_eq!(task["plan_mode_attributed_count"], 1);
    let attributed: Vec<bool> = task["denials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|denial| denial["plan_mode_attributed"].as_bool().unwrap_or(false))
        .collect();
    assert_eq!(attributed, vec![true, false]);
}
