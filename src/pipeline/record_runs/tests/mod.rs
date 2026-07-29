//! Tests for the `record-runs` stage, grouped by concern into the submodules
//! below. This root owns the fixtures they share: the harness events-file
//! writers, the `dispatch.json` builder, and the artifact readers.

use super::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use tempfile::TempDir;

mod assembly;
mod conversation;
mod permission_denials;
mod prompt_read;
mod skips;
mod warnings;

fn jsonl(lines: &[Value]) -> String {
    let body = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    format!("{body}\n")
}

fn write_codex_events(outputs_dir: &Path, final_text: &str) {
    let lines = vec![
        json!({"type": "thread.started", "timestamp": "2026-06-04T10:00:00.000Z"}),
        json!({"type": "item.completed", "timestamp": "2026-06-04T10:00:10.000Z", "item": {"id": "item_1", "type": "command_execution", "command": "bun test", "output": "ok"}}),
        json!({"type": "item.completed", "timestamp": "2026-06-04T10:00:20.000Z", "item": {"id": "item_2", "type": "agent_message", "text": final_text}}),
        json!({"type": "turn.completed", "timestamp": "2026-06-04T10:00:30.000Z", "usage": {"input_tokens": 100, "cached_input_tokens": 80, "output_tokens": 20, "reasoning_output_tokens": 5}}),
    ];
    fs::write(outputs_dir.join("codex-events.jsonl"), jsonl(&lines)).unwrap();
}

/// A `claude -p --output-format stream-json` events fixture: a `system/init`
/// line, one tool call, and a terminal `result` event carrying the final
/// text + duration + usage (there are no per-line timestamps). Tokens sum to
/// 125 (100 + 20 + 0 + 5).
fn write_claude_events(outputs_dir: &Path, final_text: &str) {
    let lines = vec![
        json!({"type": "system", "subtype": "init", "cwd": "/env"}),
        json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "bun test"}}]}}),
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}}),
        json!({"type": "result", "subtype": "success", "is_error": false, "result": final_text, "duration_ms": 30_000, "usage": {"input_tokens": 100, "output_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 5}}),
    ];
    fs::write(outputs_dir.join("claude-events.jsonl"), jsonl(&lines)).unwrap();
}

/// A `claude -p` events fixture where the agent reads its dispatch prompt:
/// a `Read` tool call whose `input.file_path` is `prompt_path`, a
/// `tool_result` carrying `read_result` (the file content on success, an
/// error string on a denied/out-of-cwd read), and a terminal `result` event.
fn write_claude_events_prompt_read(
    outputs_dir: &Path,
    prompt_path: &str,
    read_result: &str,
    final_text: &str,
) {
    let lines = vec![
        json!({"type": "system", "subtype": "init", "cwd": "/env"}),
        json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"file_path": prompt_path}}]}}),
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": read_result}]}}),
        json!({"type": "result", "subtype": "success", "is_error": false, "result": final_text, "duration_ms": 30_000, "usage": {"input_tokens": 100, "output_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 5}}),
    ];
    fs::write(outputs_dir.join("claude-events.jsonl"), jsonl(&lines)).unwrap();
}

const PROMPT_SENTINEL: &str =
    "You are executing a single test case for a skill evaluation framework.";

struct FixtureTask {
    eval_id: &'static str,
    condition: &'static str,
    /// Written to `outputs/final-message.md` when `Some`.
    final_message: Option<&'static str>,
}

/// Paths the tests reach into after building the iteration.
struct TaskPaths {
    outputs_dir: PathBuf,
    run_record_path: PathBuf,
    timing_path: PathBuf,
}

/// Build an iteration dir + `dispatch.json` shaped like `run.ts` serializes it.
fn write_iteration(iteration_dir: &Path, tasks: &[FixtureTask]) -> Vec<TaskPaths> {
    let mut serialized = Vec::new();
    let mut paths = Vec::new();
    for t in tasks {
        let cond_dir = iteration_dir
            .join(format!("eval-{}", t.eval_id))
            .join(t.condition);
        let outputs_dir = cond_dir.join("outputs");
        fs::create_dir_all(&outputs_dir).unwrap();
        if let Some(msg) = t.final_message {
            fs::write(outputs_dir.join("final-message.md"), msg).unwrap();
        }
        let run_record_path = cond_dir.join("run.json");
        let timing_path = cond_dir.join("timing.json");
        let without = t.condition == "without_skill";
        serialized.push(json!({
            "eval_id": t.eval_id,
            "condition": t.condition,
            "skill_path": if without { Value::Null } else { json!("/staged/skill/SKILL.md") },
            "staged_skill_slug": if without { Value::Null } else { json!("test-slug") },
            "user_prompt": format!("Do the {} task", t.eval_id),
            "fixtures": [cond_dir.join("inputs").join("fixture.txt").to_string_lossy()],
            "outputs_dir": outputs_dir.to_string_lossy(),
            "run_record_path": run_record_path.to_string_lossy(),
            "timing_path": timing_path.to_string_lossy(),
            "agent_description": format!("{}:{}:i1-nonce1", t.eval_id, t.condition),
            "dispatch_prompt_path": cond_dir.join("dispatch-prompt.txt").to_string_lossy(),
        }));
        paths.push(TaskPaths {
            outputs_dir,
            run_record_path,
            timing_path,
        });
    }
    fs::write(
        iteration_dir.join("dispatch.json"),
        serde_json::to_string_pretty(&json!({"run_nonce": "nonce1", "tasks": serialized})).unwrap(),
    )
    .unwrap();
    paths
}

fn read_run(iteration_dir: &Path, eval_id: &str, condition: &str) -> RunRecord {
    let raw = fs::read_to_string(
        iteration_dir
            .join(format!("eval-{eval_id}"))
            .join(condition)
            .join("run.json"),
    )
    .unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn read_timing_value(iteration_dir: &Path, eval_id: &str, condition: &str) -> Value {
    let raw = fs::read_to_string(
        iteration_dir
            .join(format!("eval-{eval_id}"))
            .join(condition)
            .join("timing.json"),
    )
    .unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn timing_exists(iteration_dir: &Path, eval_id: &str, condition: &str) -> bool {
    iteration_dir
        .join(format!("eval-{eval_id}"))
        .join(condition)
        .join("timing.json")
        .exists()
}

fn run_exists(iteration_dir: &Path, eval_id: &str, condition: &str) -> bool {
    iteration_dir
        .join(format!("eval-{eval_id}"))
        .join(condition)
        .join("run.json")
        .exists()
}

/// The iteration dir under a fresh temp root.
fn dirs(root: &TempDir) -> PathBuf {
    let iteration_dir = root.path().join("iter");
    fs::create_dir_all(&iteration_dir).unwrap();
    iteration_dir
}
