//! Claude Code `-p --output-format stream-json` transcript parsing.
//!
//! Parses the newline-delimited JSON event stream that `claude -p
//! --output-format stream-json --verbose` writes (captured per task as
//! `outputs/claude-events.jsonl`). The `assistant`/`user` events wrap a full
//! Anthropic Messages object under `message`, so tool-call extraction is shared
//! with the [`transcript`](super::transcript) record
//! types. The differences are all in the envelope: there are no per-line
//! timestamps, and a terminal `result` event carries the authoritative final
//! text, wall-clock duration, token usage, and the tool calls the CLI refused to
//! run. `system`, `rate_limit_event`, and any other non-message events are
//! ignored (they don't deserialize into an assistant/user record, so the shared
//! extractor skips them).

use std::io;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::core::ToolInvocation;

use crate::adapters::transcript::{
    PermissionDenial, TranscriptEvent, TranscriptSummary, read_jsonl,
};

use super::transcript::{
    TranscriptRecord, UsageRecord, extract_events, extract_invocations, last_assistant_text,
    tool_result_texts,
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
    /// Tool calls the CLI refused to run. Absent on builds that predate the
    /// field, empty when everything the agent asked for was permitted.
    #[serde(default)]
    permission_denials: Vec<DenialEntry>,
}

/// One refused tool call inside [`ResultEvent::permission_denials`].
#[derive(Debug, serde::Deserialize)]
struct DenialEntry {
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    tool_input: Option<Value>,
}

/// Read the stream into records, keeping the terminal `result` event separately.
/// Lines that don't shape into a record are skipped rather than failing the
/// parse.
fn read_stream(
    path: &Path,
) -> io::Result<(Vec<Value>, Vec<TranscriptRecord>, Option<ResultEvent>)> {
    let values = read_jsonl::<Value>(path)?;
    let mut records: Vec<TranscriptRecord> = Vec::new();
    let mut result_event: Option<ResultEvent> = None;
    for value in &values {
        let Ok(record) = TranscriptRecord::deserialize(value) else {
            continue;
        };
        if record.record_type.as_deref() == Some("result") {
            result_event = ResultEvent::deserialize(value).ok();
        }
        records.push(record);
    }
    Ok((values, records, result_event))
}

/// Parse the event stream into ordered tool invocations. Reuses the shared
/// extractor: non-message events deserialize into records the extractor skips.
pub fn parse_claude_stream_json(path: &Path) -> io::Result<Vec<ToolInvocation>> {
    Ok(extract_invocations(&read_jsonl::<TranscriptRecord>(path)?))
}

/// Parse the event stream into a full [`TranscriptSummary`]. Final text,
/// duration, and token totals come from the terminal `result` event; on a
/// missing or errored `result` the final text falls back to the last assistant
/// message's text, and duration/tokens fall back to `None`.
pub fn parse_claude_stream_json_full(path: &Path) -> io::Result<TranscriptSummary> {
    let (values, records, result_event) = read_stream(path)?;

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
    let session_id = values.iter().find_map(|value| {
        (value.get("type").and_then(Value::as_str) == Some("system"))
            .then(|| value.get("session_id").and_then(Value::as_str))
            .flatten()
            .map(str::to_string)
    });
    let mut events = extract_events(&records);
    if let Some(text) = &final_text {
        let last_matches = events.iter().rev().find_map(|event| match event {
            TranscriptEvent::AssistantMessage { text, .. } => Some(text),
            TranscriptEvent::ToolInvocation { .. } => None,
        }) == Some(text);
        if !last_matches {
            events.push(TranscriptEvent::AssistantMessage {
                ordinal: events.len() as u32,
                text: text.clone(),
            });
        }
    }

    Ok(TranscriptSummary {
        tool_invocations: extract_invocations(&records),
        events,
        session_id,
        total_tokens,
        duration_ms,
        final_text,
    })
}

/// Parse the tool calls the CLI refused to run. The terminal `result` event
/// reports them structurally in `permission_denials` — no refusal-text matching
/// needed — and each entry's refusal reason is recovered from the `tool_result`
/// block its `tool_use_id` points at. Eval write-guard blocks land here too: the
/// guard denies through a PreToolUse hook, which Claude Code reports like any
/// other refusal, so the reason is kept verbatim for the pipeline to attribute.
///
/// An absent `permission_denials` field (builds predating it), a run where
/// nothing was refused, and a run that never reached a `result` event all yield
/// an empty vec.
pub fn parse_claude_permission_denials(path: &Path) -> io::Result<Vec<PermissionDenial>> {
    let (_, records, result_event) = read_stream(path)?;
    let Some(result_event) = result_event else {
        return Ok(Vec::new());
    };
    if result_event.permission_denials.is_empty() {
        return Ok(Vec::new());
    }

    let reasons = tool_result_texts(&records);
    Ok(result_event
        .permission_denials
        .iter()
        .map(|entry| {
            let mut input_keys = entry
                .tool_input
                .as_ref()
                .and_then(Value::as_object)
                .map(|input| input.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            input_keys.sort();
            PermissionDenial {
                tool: entry.tool_name.clone(),
                reason: entry
                    .tool_use_id
                    .as_deref()
                    .and_then(|id| reasons.get(id))
                    .cloned(),
                input_keys,
            }
        })
        .collect())
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
                json!({"type": "system", "subtype": "init", "session_id": "session-claude-1"}),
                json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "text", "text": "working"}]}}),
                json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done", "duration_ms": 5637, "usage": usage()}),
            ],
        );

        let summary = super::parse_claude_stream_json_full(&path).unwrap();
        assert_eq!(summary.session_id.as_deref(), Some("session-claude-1"));
        assert_eq!(
            serde_json::to_value(&summary.events).unwrap(),
            json!([
                {
                    "type": "assistant_message",
                    "ordinal": 0,
                    "text": "working"
                },
                {
                    "type": "assistant_message",
                    "ordinal": 1,
                    "text": "Done"
                }
            ])
        );
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

    #[test]
    fn permission_denials_carry_tool_refusal_reason_and_sorted_input_keys() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [
                    {"type": "tool_use", "id": "toolu_ok", "name": "Read", "input": {"file_path": "/env/a.txt"}},
                    {"type": "tool_use", "id": "toolu_no", "name": "Bash", "input": {"command": "TZ='America/Los_Angeles' node -e \"1\"", "description": "shifted clock"}}
                ]}}),
                json!({"type": "user", "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_ok", "content": "contents", "is_error": false},
                    {"type": "tool_result", "tool_use_id": "toolu_no", "content": "This command requires approval", "is_error": true}
                ]}}),
                json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done", "duration_ms": 12, "usage": usage(),
                "permission_denials": [
                    {"tool_name": "Bash", "tool_use_id": "toolu_no", "tool_input": {"command": "TZ='America/Los_Angeles' node -e \"1\"", "description": "shifted clock"}}
                ]}),
            ],
        );

        let denials = super::parse_claude_permission_denials(&path).unwrap();
        assert_eq!(denials.len(), 1);
        assert_eq!(denials[0].tool, "Bash");
        assert_eq!(
            denials[0].reason.as_deref(),
            Some("This command requires approval")
        );
        assert_eq!(denials[0].input_keys, vec!["command", "description"]);
    }

    #[test]
    fn no_permission_denials_when_the_result_event_reports_none() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [
                    {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}
                ]}}),
                json!({"type": "user", "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "a.txt", "is_error": false}
                ]}}),
                // A build that predates the field omits it entirely; an allowed
                // run reports an empty array. Both mean "nothing was refused".
                json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done", "duration_ms": 12, "usage": usage()}),
            ],
        );
        assert!(
            super::parse_claude_permission_denials(&path)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn no_permission_denials_when_the_run_never_reached_a_result_event() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(&path, &[json!({"type": "system", "subtype": "init"})]);
        assert!(
            super::parse_claude_permission_denials(&path)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_denial_without_a_matching_tool_result_keeps_the_tool_and_drops_the_reason() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done", "duration_ms": 12, "usage": usage(),
                "permission_denials": [
                    {"tool_name": "Write", "tool_use_id": "toolu_absent", "tool_input": {"file_path": "/tmp/x", "content": "…"}}
                ]}),
            ],
        );

        let denials = super::parse_claude_permission_denials(&path).unwrap();
        assert_eq!(denials.len(), 1);
        assert_eq!(denials[0].tool, "Write");
        assert_eq!(denials[0].reason, None);
        assert_eq!(denials[0].input_keys, vec!["content", "file_path"]);
    }

    #[test]
    fn a_guard_blocked_call_is_reported_with_the_guard_reason_verbatim() {
        // The eval write guard denies through a PreToolUse hook, which Claude
        // Code reports in `permission_denials` like any other refusal. Keeping
        // the reason verbatim is what lets the pipeline attribute it to the
        // guard instead of double-reporting it.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "user", "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_g", "content": "eval guard: Write to /etc/passwd is outside the eval sandbox", "is_error": true}
                ]}}),
                json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done", "duration_ms": 12, "usage": usage(),
                "permission_denials": [
                    {"tool_name": "Write", "tool_use_id": "toolu_g", "tool_input": {"file_path": "/etc/passwd"}}
                ]}),
            ],
        );

        let denials = super::parse_claude_permission_denials(&path).unwrap();
        assert_eq!(denials.len(), 1);
        assert_eq!(
            denials[0].reason.as_deref(),
            Some("eval guard: Write to /etc/passwd is outside the eval sandbox")
        );
    }
}
