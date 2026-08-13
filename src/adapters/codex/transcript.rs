//! Codex transcript (event-stream) parsing.
//!
//! Codex emits a JSONL event stream;
//! `item.completed` events whose item type is not an agent message / reasoning /
//! plan update become ordered [`ToolInvocation`]s. Produces the same
//! [`TranscriptSummary`] shape as the Claude adapter, but with Codex's blended
//! token accounting: non-cached input plus output.

use crate::adapters::transcript::TranscriptEvent;
use crate::adapters::transcript::read_jsonl;
use crate::adapters::{PermissionDenial, TranscriptSummary};
use crate::core::ToolInvocation;
use serde_json::{Map, Value};
use std::fs;
use std::io;
use std::path::Path;

const NON_TOOL_ITEMS: [&str; 3] = ["agent_message", "reasoning", "plan_update"];
const ARG_OMIT_KEYS: [&str; 7] = [
    "id",
    "type",
    "status",
    "aggregated_output",
    "output",
    "result",
    "error",
];

fn permission_denial(tool: &str, reason: &str) -> PermissionDenial {
    PermissionDenial {
        tool: tool.into(),
        reason: Some(reason.into()),
        input_keys: vec!["command".into()],
    }
}

fn parse_permission_denial_line(line: &str) -> Option<PermissionDenial> {
    if !line.contains(" ERROR codex_core::tools::router: error=") {
        return None;
    }

    if let Some(hook_output) = line.split_once("Command blocked by PreToolUse hook: ") {
        let (reason, command) = hook_output.1.split_once(". Command: ")?;
        let tool = if command.starts_with("*** Begin Patch") {
            "apply_patch"
        } else {
            "Bash"
        };
        return Some(permission_denial(tool, reason));
    }

    let rejected = line.split_once("Rejected(\\\"")?.1.split_once("\\\")")?.0;
    let reason = if rejected == "approval required by policy, but AskForApproval is set to Never" {
        rejected
    } else {
        rejected.rsplit_once(" rejected: ")?.1
    };
    Some(permission_denial("Bash", reason))
}

fn stringify_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// First present of `aggregated_output`, `output`, `result`, `error` (a
/// present-but-null value still counts), stringified.
fn maybe_result(item: &Map<String, Value>) -> Option<String> {
    ["aggregated_output", "output", "result", "error"]
        .into_iter()
        .find_map(|k| item.get(k).map(stringify_value))
}

/// All item keys except the structural ones, preserving JSON key order; `None`
/// when nothing remains.
fn item_args(item: &Map<String, Value>) -> Option<Value> {
    let mut args = Map::new();
    for (key, value) in item {
        if ARG_OMIT_KEYS.contains(&key.as_str()) {
            continue;
        }
        args.insert(key.clone(), value.clone());
    }
    if args.is_empty() {
        None
    } else {
        Some(Value::Object(args))
    }
}

fn extract_invocations(records: &[Value]) -> Vec<ToolInvocation> {
    let mut invocations = Vec::new();
    for record in records {
        if record.get("type").and_then(Value::as_str) != Some("item.completed") {
            continue;
        }
        let Some(item) = record.get("item").and_then(Value::as_object) else {
            continue;
        };
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            continue;
        };
        if NON_TOOL_ITEMS.contains(&item_type) {
            continue;
        }
        let ordinal = invocations.len() as u32;
        invocations.push(ToolInvocation {
            name: item_type.to_string(),
            args: item_args(item),
            result: maybe_result(item).map(Value::String),
            ordinal,
        });
    }
    invocations
}

fn extract_events(records: &[Value]) -> Vec<TranscriptEvent> {
    let mut events = Vec::new();
    for record in records {
        if record.get("type").and_then(Value::as_str) != Some("item.completed") {
            continue;
        }
        let Some(item) = record.get("item").and_then(Value::as_object) else {
            continue;
        };
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            continue;
        };
        let ordinal = events.len() as u32;
        if item_type == "agent_message" {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                events.push(TranscriptEvent::AssistantMessage {
                    ordinal,
                    text: text.to_string(),
                });
            }
        } else if !NON_TOOL_ITEMS.contains(&item_type) {
            events.push(TranscriptEvent::ToolInvocation {
                ordinal,
                name: item_type.to_string(),
                args: item_args(item),
                result: maybe_result(item).map(Value::String),
            });
        }
    }
    events
}

/// Parse a Codex event stream into ordered tool invocations.
pub fn parse_codex_events(jsonl_path: &Path) -> io::Result<Vec<ToolInvocation>> {
    Ok(extract_invocations(&read_jsonl::<Value>(jsonl_path)?))
}

/// Parse permission denials from the stderr capture beside a Codex event stream.
pub fn parse_codex_permission_denials(jsonl_path: &Path) -> io::Result<Vec<PermissionDenial>> {
    let Some(filename) = jsonl_path.file_name().and_then(|name| name.to_str()) else {
        return Ok(Vec::new());
    };
    let Some(prefix) = filename.strip_suffix("events.jsonl") else {
        return Ok(Vec::new());
    };
    let stderr_path = jsonl_path.with_file_name(format!("{prefix}stderr.log"));
    let stderr = match fs::read_to_string(stderr_path) {
        Ok(stderr) => stderr,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    Ok(stderr
        .lines()
        .filter_map(parse_permission_denial_line)
        .collect())
}

fn parse_millis(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Parse a Codex event stream into a full [`TranscriptSummary`].
pub fn parse_codex_events_full(jsonl_path: &Path) -> io::Result<TranscriptSummary> {
    let records = read_jsonl::<Value>(jsonl_path)?;

    let mut first_ts: Option<i64> = None;
    let mut last_ts: Option<i64> = None;
    let mut timestamp_count = 0usize;
    let mut final_text: Option<String> = None;
    let mut total_tokens: Option<i64> = None;
    let session_id = records.iter().find_map(|record| {
        (record.get("type").and_then(Value::as_str) == Some("thread.started"))
            .then(|| record.get("thread_id").and_then(Value::as_str))
            .flatten()
            .map(str::to_string)
    });

    for record in &records {
        if let Some(ts_str) = record.get("timestamp").and_then(Value::as_str)
            && let Some(ts) = parse_millis(ts_str)
        {
            if first_ts.is_none() {
                first_ts = Some(ts);
            }
            last_ts = Some(ts);
            timestamp_count += 1;
        }

        let rtype = record.get("type").and_then(Value::as_str);

        if rtype == Some("item.completed")
            && let Some(item) = record.get("item").and_then(Value::as_object)
            && item.get("type").and_then(Value::as_str) == Some("agent_message")
            && let Some(text) = item.get("text").and_then(Value::as_str)
        {
            final_text = Some(text.to_string());
        }

        if rtype == Some("turn.completed")
            && let Some(usage) = record.get("usage").and_then(Value::as_object)
            && (usage.contains_key("input_tokens") || usage.contains_key("output_tokens"))
        {
            let get = |k: &str| usage.get(k).and_then(Value::as_i64).unwrap_or(0);
            let blended = get("input_tokens")
                .saturating_add(get("output_tokens"))
                .saturating_sub(get("cached_input_tokens"))
                .max(0);
            total_tokens = Some(total_tokens.unwrap_or(0) + blended);
        }
    }

    let duration_ms = match (first_ts, last_ts) {
        (Some(f), Some(l)) if timestamp_count >= 2 => Some(l - f),
        _ => None,
    };

    Ok(TranscriptSummary {
        tool_invocations: extract_invocations(&records),
        events: extract_events(&records),
        session_id,
        total_tokens,
        duration_ms,
        final_text,
    })
}

#[cfg(test)]
#[path = "transcript/permission_denials_tests.rs"]
mod permission_denials_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ToolInvocation;
    use serde_json::{Value, json};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_jsonl(path: &Path, lines: &[Value]) {
        let body = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{body}\n")).unwrap();
    }

    #[test]
    fn extracts_completed_tool_items_with_ordinals_args_and_results() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("items.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "item.started", "timestamp": "2026-06-07T10:00:00.000Z", "item": {"id": "item_1", "type": "command_execution", "command": "bash -lc 'bun test'", "status": "in_progress"}}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:02.000Z", "item": {"id": "item_1", "type": "command_execution", "command": "bash -lc 'bun test'", "aggregated_output": "2 pass\n0 fail", "status": "completed"}}),
                json!({"type": "item.completed", "item": {"id": "item_2", "type": "file_change", "path": "src/app.ts", "status": "completed"}}),
                json!({"type": "item.completed", "item": {"id": "item_3", "type": "agent_message", "text": "Done."}}),
            ],
        );

        let result = parse_codex_events(&path).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0],
            ToolInvocation {
                name: "command_execution".into(),
                args: Some(json!({"command": "bash -lc 'bun test'"})),
                result: Some(Value::String("2 pass\n0 fail".into())),
                ordinal: 0,
            }
        );
        assert_eq!(
            result[1],
            ToolInvocation {
                name: "file_change".into(),
                args: Some(json!({"path": "src/app.ts"})),
                result: None,
                ordinal: 1,
            }
        );
    }

    #[test]
    fn prefers_aggregated_output_and_retains_legacy_result_fallbacks() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("result-fields.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "item.completed", "item": {"id": "a", "type": "command_execution", "command": "one", "aggregated_output": "current", "output": "legacy-output", "result": "legacy-result", "error": "legacy-error", "status": "completed"}}),
                json!({"type": "item.completed", "item": {"id": "b", "type": "command_execution", "command": "two", "output": "legacy-output"}}),
                json!({"type": "item.completed", "item": {"id": "c", "type": "mcp_tool_call", "tool": "demo", "result": "legacy-result"}}),
                json!({"type": "item.completed", "item": {"id": "d", "type": "mcp_tool_call", "tool": "demo", "error": "legacy-error"}}),
            ],
        );

        let result = parse_codex_events(&path).unwrap();
        assert_eq!(
            result
                .iter()
                .map(|invocation| invocation.result.clone())
                .collect::<Vec<_>>(),
            vec![
                Some(json!("current")),
                Some(json!("legacy-output")),
                Some(json!("legacy-result")),
                Some(json!("legacy-error")),
            ]
        );
        assert_eq!(result[0].args, Some(json!({"command": "one"})));
    }

    #[test]
    fn skips_malformed_jsonl_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("malformed.jsonl");
        let good = json!({"type": "item.completed", "item": {"id": "item_1", "type": "web_search", "query": "codex exec json"}});
        fs::write(&path, format!("{good}\nnot valid json\n")).unwrap();
        assert_eq!(
            parse_codex_events(&path).unwrap(),
            vec![ToolInvocation {
                name: "web_search".into(),
                args: Some(json!({"query": "codex exec json"})),
                result: None,
                ordinal: 0,
            }]
        );
    }

    #[test]
    fn preserves_text_fields_on_non_message_tool_items() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tool-text.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "item.completed", "item": {"id": "item_1", "type": "web_search", "query": "codex events", "text": "search summary"}}),
            ],
        );
        assert_eq!(
            parse_codex_events(&path).unwrap(),
            vec![ToolInvocation {
                name: "web_search".into(),
                args: Some(json!({"query": "codex events", "text": "search summary"})),
                result: None,
                ordinal: 0,
            }]
        );
    }

    #[test]
    fn does_not_treat_agent_messages_reasoning_or_plan_updates_as_tools() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("non-tools.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "item.completed", "item": {"id": "a", "type": "agent_message"}}),
                json!({"type": "item.completed", "item": {"id": "b", "type": "reasoning"}}),
                json!({"type": "item.completed", "item": {"id": "c", "type": "plan_update"}}),
            ],
        );
        assert_eq!(parse_codex_events(&path).unwrap(), vec![]);
    }

    #[test]
    fn extracts_invocations_last_agent_text_usage_and_duration() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("full.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "thread.started", "thread_id": "thread-codex-1", "timestamp": "2026-06-07T10:00:00.000Z"}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:03.000Z", "item": {"id": "item_1", "type": "command_execution", "command": "ls", "aggregated_output": "README.md"}}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:04.000Z", "item": {"id": "item_2", "type": "agent_message", "text": "First."}}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:05.000Z", "item": {"id": "item_3", "type": "agent_message", "text": "Final."}}),
                json!({"type": "turn.completed", "timestamp": "2026-06-07T10:00:10.000Z", "usage": {"input_tokens": 100, "cached_input_tokens": 75, "output_tokens": 20, "reasoning_output_tokens": 5}}),
            ],
        );

        let full = parse_codex_events_full(&path).unwrap();
        assert_eq!(
            full.tool_invocations,
            vec![ToolInvocation {
                name: "command_execution".into(),
                args: Some(json!({"command": "ls"})),
                result: Some(Value::String("README.md".into())),
                ordinal: 0,
            }]
        );
        assert_eq!(full.session_id.as_deref(), Some("thread-codex-1"));
        assert_eq!(
            serde_json::to_value(&full.events).unwrap(),
            json!([
                {
                    "type": "tool_invocation",
                    "ordinal": 0,
                    "name": "command_execution",
                    "args": {"command": "ls"},
                    "result": "README.md"
                },
                {
                    "type": "assistant_message",
                    "ordinal": 1,
                    "text": "First."
                },
                {
                    "type": "assistant_message",
                    "ordinal": 2,
                    "text": "Final."
                }
            ])
        );
        assert_eq!(full.final_text, Some("Final.".into()));
        assert_eq!(full.total_tokens, Some(45)); // non-cached input plus output
        assert_eq!(full.duration_ms, Some(10_000));
    }

    #[test]
    fn reports_codex_blended_usage_without_double_counting_reasoning() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("codex-usage.jsonl");
        write_jsonl(
            &path,
            &[json!({
                "type": "turn.completed",
                "usage": {
                    "input_tokens": 886_850,
                    "cached_input_tokens": 833_024,
                    "output_tokens": 10_083,
                    "reasoning_output_tokens": 6_070
                }
            })],
        );

        let full = parse_codex_events_full(&path).unwrap();
        assert_eq!(full.total_tokens, Some(63_909));
        assert_eq!(full.duration_ms, None);
    }

    #[test]
    fn turn_without_input_or_output_leaves_tokens_null() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cached-only-usage.jsonl");
        write_jsonl(
            &path,
            &[json!({
                "type": "turn.completed",
                "usage": {"cached_input_tokens": 100}
            })],
        );

        let full = parse_codex_events_full(&path).unwrap();
        assert_eq!(full.total_tokens, None);
    }

    #[test]
    fn returns_null_usage_and_duration_when_sparse() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sparse.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:00.000Z", "item": {"id": "item_1", "type": "agent_message", "text": "Done."}}),
            ],
        );
        let full = parse_codex_events_full(&path).unwrap();
        assert_eq!(full.final_text, Some("Done.".into()));
        assert_eq!(full.total_tokens, None);
        assert_eq!(full.duration_ms, None);
    }
}
