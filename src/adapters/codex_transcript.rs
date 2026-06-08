//! Codex transcript (event-stream) parsing.
//!
//! Ports `src/adapters/codex-transcript.ts`. Codex emits a JSONL event stream;
//! `item.completed` events whose item type is not an agent message / reasoning /
//! plan update become ordered [`ToolInvocation`]s. Produces the same
//! [`TranscriptSummary`] shape as the Claude adapter, but with Codex's token
//! accounting (excludes cached input tokens).

use crate::adapters::claude_code_transcript::TranscriptSummary;
use crate::core::ToolInvocation;
use serde_json::{Map, Value};
use std::fs;
use std::io;
use std::path::Path;

const NON_TOOL_ITEMS: [&str; 3] = ["agent_message", "reasoning", "plan_update"];
const ARG_OMIT_KEYS: [&str; 6] = ["id", "type", "status", "output", "result", "error"];

fn read_events(jsonl_path: &Path) -> io::Result<Vec<Value>> {
    let raw = fs::read_to_string(jsonl_path)?;
    let mut out = Vec::new();
    for line in raw.split('\n') {
        if line.is_empty() {
            continue;
        }
        // Skip malformed lines rather than failing the whole parse.
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            out.push(v);
        }
    }
    Ok(out)
}

fn stringify_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// First present of `output`, `result`, `error` (a present-but-null value still
/// counts), stringified.
fn maybe_result(item: &Map<String, Value>) -> Option<String> {
    ["output", "result", "error"]
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

/// Parse a Codex event stream into ordered tool invocations.
pub fn parse_codex_events(jsonl_path: &Path) -> io::Result<Vec<ToolInvocation>> {
    Ok(extract_invocations(&read_events(jsonl_path)?))
}

fn parse_millis(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Parse a Codex event stream into a full [`TranscriptSummary`].
pub fn parse_codex_events_full(jsonl_path: &Path) -> io::Result<TranscriptSummary> {
    let records = read_events(jsonl_path)?;

    let mut first_ts: Option<i64> = None;
    let mut last_ts: Option<i64> = None;
    let mut timestamp_count = 0usize;
    let mut final_text: Option<String> = None;
    let mut total_tokens: Option<i64> = None;

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
        {
            let get = |k: &str| usage.get(k).and_then(Value::as_i64).unwrap_or(0);
            let sum = get("input_tokens") + get("output_tokens") + get("reasoning_output_tokens");
            total_tokens = Some(total_tokens.unwrap_or(0) + sum);
        }
    }

    let duration_ms = match (first_ts, last_ts) {
        (Some(f), Some(l)) if timestamp_count >= 2 => Some(l - f),
        _ => None,
    };

    Ok(TranscriptSummary {
        tool_invocations: extract_invocations(&records),
        total_tokens,
        duration_ms,
        final_text,
    })
}

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
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:02.000Z", "item": {"id": "item_1", "type": "command_execution", "command": "bash -lc 'bun test'", "output": "2 pass\n0 fail", "status": "completed"}}),
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
                json!({"type": "thread.started", "timestamp": "2026-06-07T10:00:00.000Z"}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:03.000Z", "item": {"id": "item_1", "type": "command_execution", "command": "ls", "output": "README.md"}}),
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
        assert_eq!(full.final_text, Some("Final.".into()));
        assert_eq!(full.total_tokens, Some(125)); // cached input excluded
        assert_eq!(full.duration_ms, Some(10_000));
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
