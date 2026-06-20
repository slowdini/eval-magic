//! Claude Code `-p --output-format stream-json` transcript parsing.
//!
//! Parses the newline-delimited JSON event stream that `claude -p
//! --output-format stream-json --verbose` writes (captured per task as
//! `outputs/claude-events.jsonl`). The `assistant`/`user` events wrap a full
//! Anthropic Messages object under `message`, so tool-call extraction is shared
//! with the in-session [`claude_code_transcript`](super::claude_code_transcript)
//! parser. The differences are all in the envelope: there are no per-line
//! timestamps, and a terminal `result` event carries the authoritative final
//! text, wall-clock duration, and token usage. `system`, `rate_limit_event`, and
//! any other non-message events are ignored (they don't deserialize into an
//! assistant/user record, so the shared extractor skips them).

use std::fs;
use std::io;
use std::path::Path;

use crate::core::ToolInvocation;

use super::TranscriptSummary;
use super::claude_code_transcript::{
    TranscriptRecord, UsageRecord, extract_invocations, last_assistant_text, read_records,
};

/// The terminal `{"type":"result", …}` event of a `-p` stream-json run.
#[derive(Debug, serde::Deserialize)]
struct ResultEvent {
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    duration_ms: Option<i64>,
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    usage: Option<UsageRecord>,
}

/// Parse the event stream into ordered tool invocations. Reuses the in-session
/// extractor: non-message events deserialize into records the extractor skips.
pub fn parse_claude_stream_json(path: &Path) -> io::Result<Vec<ToolInvocation>> {
    Ok(extract_invocations(&read_records(path)?))
}

/// Parse the event stream into a full [`TranscriptSummary`]. Final text,
/// duration, and token totals come from the terminal `result` event; on a
/// missing or errored `result` the final text falls back to the last assistant
/// message's text, and duration/tokens fall back to `None`.
pub fn parse_claude_stream_json_full(path: &Path) -> io::Result<TranscriptSummary> {
    let raw = fs::read_to_string(path)?;
    let mut records: Vec<TranscriptRecord> = Vec::new();
    let mut result_event: Option<ResultEvent> = None;
    for line in raw.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        // Skip malformed lines rather than failing the whole parse.
        let Ok(record) = serde_json::from_str::<TranscriptRecord>(line) else {
            continue;
        };
        if record.record_type.as_deref() == Some("result") {
            result_event = serde_json::from_str::<ResultEvent>(line).ok();
        }
        records.push(record);
    }

    let total_tokens = result_event
        .as_ref()
        .and_then(|e| e.usage.as_ref())
        .map(|u| {
            u.input_tokens.unwrap_or(0)
                + u.output_tokens.unwrap_or(0)
                + u.cache_creation_input_tokens.unwrap_or(0)
                + u.cache_read_input_tokens.unwrap_or(0)
        });
    let duration_ms = result_event.as_ref().and_then(|e| e.duration_ms);
    let final_text = match &result_event {
        Some(ev) if ev.is_error != Some(true) => {
            ev.result.clone().or_else(|| last_assistant_text(&records))
        }
        _ => last_assistant_text(&records),
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

    fn usage() -> Value {
        json!({
            "input_tokens": 4932,
            "output_tokens": 139,
            "cache_creation_input_tokens": 8287,
            "cache_read_input_tokens": 33490,
        })
    }

    #[test]
    fn extracts_tool_use_and_result_skipping_non_message_events() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "system", "subtype": "init", "cwd": "/env", "model": "claude-opus-4-8", "tools": ["Bash"]}),
                json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "usage": usage(), "content": [
                    {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}
                ]}}),
                json!({"type": "rate_limit_event", "rate_limit_info": {}}),
                json!({"type": "user", "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "a.txt"}
                ]}}),
                json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done", "duration_ms": 5637, "usage": usage()}),
            ],
        );

        let result = super::parse_claude_stream_json(&path).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Bash");
        assert_eq!(result[0].ordinal, 0);
        assert_eq!(result[0].args, Some(json!({"command": "ls"})));
        assert_eq!(result[0].result, Some(Value::String("a.txt".into())));
    }

    #[test]
    fn result_event_supplies_final_text_duration_and_tokens() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "system", "subtype": "init"}),
                json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "text", "text": "working"}]}}),
                json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done", "duration_ms": 5637, "usage": usage()}),
            ],
        );

        let summary = super::parse_claude_stream_json_full(&path).unwrap();
        assert_eq!(summary.final_text, Some("Done".into()));
        assert_eq!(summary.duration_ms, Some(5637));
        // 4932 + 139 + 8287 + 33490
        assert_eq!(summary.total_tokens, Some(46848));
    }

    #[test]
    fn skill_tool_use_is_preserved_for_meta_check() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [
                    {"type": "tool_use", "id": "toolu_s", "name": "Skill", "input": {"command": "slow-powers-eval-mr-review"}}
                ]}}),
                json!({"type": "result", "subtype": "success", "is_error": false, "result": "ok", "duration_ms": 10, "usage": usage()}),
            ],
        );
        let result = super::parse_claude_stream_json(&path).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Skill");
        assert_eq!(
            result[0].args,
            Some(json!({"command": "slow-powers-eval-mr-review"}))
        );
    }

    #[test]
    fn final_text_falls_back_to_last_assistant_text_when_result_is_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "text", "text": "partial work"}]}}),
                json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "result": "Execution error", "duration_ms": 12, "usage": usage()}),
            ],
        );
        let summary = super::parse_claude_stream_json_full(&path).unwrap();
        assert_eq!(summary.final_text, Some("partial work".into()));
    }

    #[test]
    fn skips_malformed_jsonl_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        let good = json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"file_path": "/tmp/x"}}
        ]}});
        let result_line = json!({"type": "result", "is_error": false, "result": "Done", "duration_ms": 1, "usage": usage()});
        let body = format!("{good}\nnot valid json\n{result_line}\n");
        fs::write(&path, body).unwrap();

        let result = super::parse_claude_stream_json(&path).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Read");
    }

    #[test]
    fn null_duration_and_tokens_when_no_result_event() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "system", "subtype": "init"}),
                json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "text", "text": "incomplete"}]}}),
            ],
        );
        let summary = super::parse_claude_stream_json_full(&path).unwrap();
        assert_eq!(summary.duration_ms, None);
        assert_eq!(summary.total_tokens, None);
        assert_eq!(summary.final_text, Some("incomplete".into()));
    }
}
