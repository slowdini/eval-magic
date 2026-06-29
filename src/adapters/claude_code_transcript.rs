//! Claude Code transcript record types and tool-call extraction.
//!
//! Defines the JSONL record shapes and the shared extractors — ordered
//! [`ToolInvocation`]s (matching `tool_result` blocks back to their `tool_use` by
//! id) and the last assistant text — reused by the `claude -p` stream-json parser
//! ([`claude_stream_json`](super::claude_stream_json)), plus the
//! [`TranscriptSummary`] the pipeline consumes.

use crate::core::ToolInvocation;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub(crate) struct UsageRecord {
    pub(crate) input_tokens: Option<i64>,
    pub(crate) output_tokens: Option<i64>,
    pub(crate) cache_creation_input_tokens: Option<i64>,
    pub(crate) cache_read_input_tokens: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Message {
    /// String or array of content blocks; inspected as raw JSON.
    content: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TranscriptRecord {
    #[serde(rename = "type")]
    pub(crate) record_type: Option<String>,
    message: Option<Message>,
}

/// Content blocks of a message: the array as-is, or empty for string/absent
/// content.
fn content_blocks(message: &Option<Message>) -> &[Value] {
    match message.as_ref().and_then(|m| m.content.as_ref()) {
        Some(Value::Array(arr)) => arr,
        _ => &[],
    }
}

/// Coerce a JSON value to a plain string the way JS `String(x)` would for the
/// common cases (a `tool_result` array element's `text` field).
fn value_to_plain_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// Stringify a `tool_result` block's content: strings pass through; arrays join
/// their elements with newlines (string as-is, object's `text` field coerced,
/// else JSON); other values are JSON-encoded.
fn stringify_result(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .map(|c| match c {
                Value::String(s) => s.clone(),
                Value::Object(_) => match c.get("text") {
                    Some(t) => value_to_plain_string(t),
                    None => serde_json::to_string(c).unwrap_or_default(),
                },
                _ => serde_json::to_string(c).unwrap_or_default(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
        None => String::new(),
    }
}

pub(crate) fn read_records(jsonl_path: &Path) -> io::Result<Vec<TranscriptRecord>> {
    let raw = fs::read_to_string(jsonl_path)?;
    let mut records = Vec::new();
    for line in raw.split('\n') {
        if line.is_empty() {
            continue;
        }
        // Skip malformed lines rather than failing the whole parse.
        if let Ok(rec) = serde_json::from_str::<TranscriptRecord>(line) {
            records.push(rec);
        }
    }
    Ok(records)
}

pub(crate) fn extract_invocations(records: &[TranscriptRecord]) -> Vec<ToolInvocation> {
    let mut invocations: Vec<ToolInvocation> = Vec::new();
    let mut index_by_id: HashMap<String, usize> = HashMap::new();

    for record in records {
        let rtype = record.record_type.as_deref();
        let blocks = content_blocks(&record.message);

        if rtype == Some("assistant") {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                let ordinal = invocations.len();
                if let Some(id) = block.get("id").and_then(Value::as_str) {
                    index_by_id.insert(id.to_string(), ordinal);
                }
                invocations.push(ToolInvocation {
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    args: block.get("input").cloned(),
                    result: None,
                    ordinal: ordinal as u32,
                });
            }
        } else if rtype == Some("user") {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                    continue;
                }
                let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
                    continue;
                };
                let Some(&idx) = index_by_id.get(id) else {
                    continue;
                };
                invocations[idx].result =
                    Some(Value::String(stringify_result(block.get("content"))));
            }
        }
    }

    invocations
}

/// The concatenated text blocks of the last assistant message carrying any text.
/// Shared with the `-p` stream-json parser, which uses it as the final-message
/// fallback when the terminal `result` event is absent or errored.
pub(crate) fn last_assistant_text(records: &[TranscriptRecord]) -> Option<String> {
    let mut final_text: Option<String> = None;
    for record in records {
        if record.record_type.as_deref() != Some("assistant") {
            continue;
        }
        let texts: Vec<&str> = content_blocks(&record.message)
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect();
        if !texts.is_empty() {
            final_text = Some(texts.join("\n"));
        }
    }
    final_text
}

/// A transcript boiled down to the artifacts the pipeline needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSummary {
    pub tool_invocations: Vec<ToolInvocation>,
    /// Total token usage (input + output + cache creation/read), as reported by
    /// the run's terminal `result` event.
    pub total_tokens: Option<i64>,
    /// Wall-clock duration, as reported by the run's terminal `result` event.
    pub duration_ms: Option<i64>,
    /// Concatenated text blocks of the last assistant message.
    pub final_text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// Read the records and run the shared tool-call extractor — the path the
    /// stream-json parser also takes.
    fn invocations(path: &Path) -> Vec<ToolInvocation> {
        extract_invocations(&read_records(path).unwrap())
    }

    #[test]
    fn extracts_tool_use_blocks_with_ordinal_and_args() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("simple.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "user", "message": {"role": "user", "content": "Run the tests"}}),
                json!({"type": "assistant", "message": {"role": "assistant", "content": [
                    {"type": "text", "text": "Running tests now."},
                    {"type": "tool_use", "id": "toolu_001", "name": "Bash", "input": {"command": "bun test"}}
                ]}}),
                json!({"type": "user", "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_001", "content": "2 pass\n0 fail"}
                ]}}),
                json!({"type": "assistant", "message": {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "toolu_002", "name": "Read", "input": {"file_path": "/tmp/x.txt"}}
                ]}}),
            ],
        );

        let result = invocations(&path);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "Bash");
        assert_eq!(result[0].ordinal, 0);
        assert_eq!(result[0].args, Some(json!({"command": "bun test"})));
        assert_eq!(
            result[0].result,
            Some(Value::String("2 pass\n0 fail".into()))
        );
        assert_eq!(result[1].name, "Read");
        assert_eq!(result[1].ordinal, 1);
        assert_eq!(result[1].args, Some(json!({"file_path": "/tmp/x.txt"})));
        assert_eq!(result[1].result, None);
    }

    #[test]
    fn returns_empty_when_no_tool_use_blocks() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("no-tools.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "user", "message": {"role": "user", "content": "hi"}}),
                json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "hello"}]}}),
            ],
        );
        assert_eq!(invocations(&path), vec![]);
    }

    #[test]
    fn skips_malformed_jsonl_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("malformed.jsonl");
        let good_a = json!({"type": "assistant", "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_a", "name": "Bash", "input": {"command": "ls"}}
        ]}});
        let good_b = json!({"type": "assistant", "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_b", "name": "Read", "input": {"file_path": "/tmp"}}
        ]}});
        let body = format!("{good_a}\nnot valid json\n{good_b}\n");
        fs::write(&path, body).unwrap();

        let result = invocations(&path);
        assert_eq!(result.len(), 2);
        assert_eq!(
            result.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["Bash", "Read"]
        );
    }

    #[test]
    fn handles_tool_result_with_array_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("array-result.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "assistant", "message": {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "toolu_x", "name": "Bash", "input": {"command": "echo hi"}}
                ]}}),
                json!({"type": "user", "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_x", "content": [{"type": "text", "text": "hi"}]}
                ]}}),
            ],
        );
        let result = invocations(&path);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].result, Some(Value::String("hi".into())));
    }

    #[test]
    fn last_assistant_text_concatenates_text_of_last_assistant_message() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("final-text.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "text", "text": "intermediate"}]}}),
                json!({"type": "assistant", "message": {"id": "msg_2", "role": "assistant", "content": [
                    {"type": "text", "text": "All tests pass."},
                    {"type": "tool_use", "id": "toolu_z", "name": "Bash", "input": {"command": "true"}},
                    {"type": "text", "text": "Wrapping up."}
                ]}}),
                json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_z", "content": "ok"}]}}),
            ],
        );
        assert_eq!(
            last_assistant_text(&read_records(&path).unwrap()),
            Some("All tests pass.\nWrapping up.".into())
        );
    }

    #[test]
    fn last_assistant_text_is_null_when_no_assistant_text() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("no-text.jsonl");
        write_jsonl(
            &path,
            &[json!({"type": "user", "message": {"role": "user", "content": "hi"}})],
        );
        assert_eq!(last_assistant_text(&read_records(&path).unwrap()), None);
    }
}
