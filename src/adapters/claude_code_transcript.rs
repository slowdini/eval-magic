//! Claude Code transcript parsing.
//!
//! Ports `src/adapters/claude-code-transcript.ts`. Reads a JSONL session
//! transcript and extracts ordered [`ToolInvocation`]s (matching `tool_result`
//! blocks back to their `tool_use` by id), plus a [`TranscriptSummary`] with
//! deduped token totals, wall-clock duration, and the final assistant text.
//! Also resolves subagent transcripts by their `.meta.json` description.

use crate::core::ToolInvocation;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Deserialize)]
struct UsageRecord {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Message {
    id: Option<String>,
    usage: Option<UsageRecord>,
    /// String or array of content blocks; inspected as raw JSON.
    content: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct TranscriptRecord {
    #[serde(rename = "type")]
    record_type: Option<String>,
    timestamp: Option<String>,
    message: Option<Message>,
}

/// Content blocks of a message: the array as-is, or empty for string/absent
/// content (mirrors the TS `flattenContent`).
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

fn read_records(jsonl_path: &Path) -> io::Result<Vec<TranscriptRecord>> {
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

fn extract_invocations(records: &[TranscriptRecord]) -> Vec<ToolInvocation> {
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

/// Parse the transcript at `jsonl_path` into ordered tool invocations.
pub fn parse_transcript(jsonl_path: &Path) -> io::Result<Vec<ToolInvocation>> {
    Ok(extract_invocations(&read_records(jsonl_path)?))
}

/// A transcript boiled down to the artifacts the pipeline needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSummary {
    pub tool_invocations: Vec<ToolInvocation>,
    /// Sum of usage across unique API responses (deduped by `message.id`).
    /// Includes cache creation/read tokens — a different accounting than the
    /// harness's task-completion event.
    pub total_tokens: Option<i64>,
    /// Wall clock between the first and last line timestamps.
    pub duration_ms: Option<i64>,
    /// Concatenated text blocks of the last assistant message.
    pub final_text: Option<String>,
}

fn parse_millis(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Parse the transcript into a full [`TranscriptSummary`].
pub fn parse_transcript_full(jsonl_path: &Path) -> io::Result<TranscriptSummary> {
    let records = read_records(jsonl_path)?;

    let mut usage_by_id: HashMap<&str, &UsageRecord> = HashMap::new();
    let mut first_ts: Option<i64> = None;
    let mut last_ts: Option<i64> = None;
    let mut timestamp_count = 0usize;
    let mut final_text: Option<String> = None;

    for record in &records {
        if let Some(ts_str) = &record.timestamp
            && let Some(ts) = parse_millis(ts_str)
        {
            if first_ts.is_none() {
                first_ts = Some(ts);
            }
            last_ts = Some(ts);
            timestamp_count += 1;
        }

        if record.record_type.as_deref() != Some("assistant") {
            continue;
        }

        if let Some(msg) = &record.message
            && let (Some(id), Some(usage)) = (&msg.id, &msg.usage)
        {
            usage_by_id.insert(id, usage);
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

    let total_tokens = if usage_by_id.is_empty() {
        None
    } else {
        Some(
            usage_by_id
                .values()
                .map(|u| {
                    u.input_tokens.unwrap_or(0)
                        + u.output_tokens.unwrap_or(0)
                        + u.cache_creation_input_tokens.unwrap_or(0)
                        + u.cache_read_input_tokens.unwrap_or(0)
                })
                .sum(),
        )
    };

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

/// Metadata sidecar (`<base>.meta.json`) written alongside a subagent transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentMeta {
    #[serde(rename = "agentType", skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "toolUseId", skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
}

/// A discovered subagent transcript and its metadata sidecar.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentEntry {
    pub jsonl_path: PathBuf,
    pub meta_path: PathBuf,
    pub meta: SubagentMeta,
}

/// List subagent transcripts (each a `<base>.meta.json` with a sibling
/// `<base>.jsonl`) under `subagents_dir`. Returns `[]` if the dir is missing.
pub fn list_subagents(subagents_dir: &Path) -> Vec<SubagentEntry> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(subagents_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        let Some(base) = name.strip_suffix(".meta.json") else {
            continue;
        };
        let meta_path = subagents_dir.join(file_name.as_os_str());
        let jsonl_path = subagents_dir.join(format!("{base}.jsonl"));
        if !jsonl_path.exists() {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&meta_path) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<SubagentMeta>(&raw) else {
            continue;
        };
        out.push(SubagentEntry {
            jsonl_path,
            meta_path,
            meta,
        });
    }
    out
}

fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Find the subagent whose meta `description` matches. On duplicates (a retry
/// within the same run), returns the most-recently-written transcript.
pub fn find_by_description(subagents_dir: &Path, description: &str) -> Option<SubagentEntry> {
    let mut matches: Vec<SubagentEntry> = list_subagents(subagents_dir)
        .into_iter()
        .filter(|e| e.meta.description.as_deref() == Some(description))
        .collect();
    if matches.len() <= 1 {
        return matches.pop();
    }
    matches.sort_by_key(|e| std::cmp::Reverse(mtime(&e.jsonl_path)));
    matches.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::fs::{self, File};
    use std::path::Path;
    use std::time::{Duration, SystemTime};
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

        let result = parse_transcript(&path).unwrap();
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
        assert_eq!(parse_transcript(&path).unwrap(), vec![]);
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

        let result = parse_transcript(&path).unwrap();
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
        let result = parse_transcript(&path).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].result, Some(Value::String("hi".into())));
    }

    fn usage(output: i64) -> Value {
        json!({
            "input_tokens": 100,
            "cache_creation_input_tokens": 50,
            "cache_read_input_tokens": 200,
            "output_tokens": output,
        })
    }

    #[test]
    fn sums_usage_across_unique_message_ids() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("full-dedup.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "user", "timestamp": "2026-06-04T10:00:00.000Z", "message": {"role": "user", "content": "go"}}),
                json!({"type": "assistant", "timestamp": "2026-06-04T10:00:05.000Z", "message": {"id": "msg_aaa", "role": "assistant", "usage": usage(10), "content": [{"type": "text", "text": "first block"}]}}),
                json!({"type": "assistant", "timestamp": "2026-06-04T10:00:06.000Z", "message": {"id": "msg_aaa", "role": "assistant", "usage": usage(10), "content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}]}}),
                json!({"type": "assistant", "timestamp": "2026-06-04T10:01:00.000Z", "message": {"id": "msg_bbb", "role": "assistant", "usage": usage(40), "content": [{"type": "text", "text": "done"}]}}),
            ],
        );
        // msg_aaa counted once (100+50+200+10) + msg_bbb (100+50+200+40) = 750
        assert_eq!(
            parse_transcript_full(&path).unwrap().total_tokens,
            Some(750)
        );
    }

    #[test]
    fn returns_null_total_tokens_when_no_usage() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("full-no-usage.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "hi"}]}}),
            ],
        );
        assert_eq!(parse_transcript_full(&path).unwrap().total_tokens, None);
    }

    #[test]
    fn derives_duration_from_first_and_last_timestamps() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("full-duration.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "user", "timestamp": "2026-06-04T10:00:00.000Z", "message": {"role": "user", "content": "go"}}),
                json!({"type": "assistant", "timestamp": "2026-06-04T10:02:30.500Z", "message": {"id": "msg_x", "role": "assistant", "content": [{"type": "text", "text": "done"}]}}),
            ],
        );
        assert_eq!(
            parse_transcript_full(&path).unwrap().duration_ms,
            Some(150_500)
        );
    }

    #[test]
    fn returns_null_duration_with_fewer_than_two_timestamps() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("full-one-ts.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "assistant", "timestamp": "2026-06-04T10:00:00.000Z", "message": {"role": "assistant", "content": []}}),
                json!({"type": "assistant", "message": {"role": "assistant", "content": []}}),
            ],
        );
        assert_eq!(parse_transcript_full(&path).unwrap().duration_ms, None);
    }

    #[test]
    fn final_text_is_concatenated_text_of_last_assistant_message() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("full-final-text.jsonl");
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
            parse_transcript_full(&path).unwrap().final_text,
            Some("All tests pass.\nWrapping up.".into())
        );
    }

    #[test]
    fn final_text_is_null_when_no_assistant_text() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("full-no-text.jsonl");
        write_jsonl(
            &path,
            &[json!({"type": "user", "message": {"role": "user", "content": "hi"}})],
        );
        assert_eq!(parse_transcript_full(&path).unwrap().final_text, None);
    }

    #[test]
    fn tool_invocations_matches_parse_transcript() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("full-invocations.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "assistant", "timestamp": "2026-06-04T10:00:00.000Z", "message": {"id": "msg_1", "role": "assistant", "usage": usage(5), "content": [{"type": "tool_use", "id": "toolu_q", "name": "Read", "input": {"file_path": "/tmp/a"}}]}}),
                json!({"type": "user", "timestamp": "2026-06-04T10:00:02.000Z", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_q", "content": "contents"}]}}),
            ],
        );
        assert_eq!(
            parse_transcript_full(&path).unwrap().tool_invocations,
            parse_transcript(&path).unwrap()
        );
    }

    #[test]
    fn matches_subagents_by_meta_description() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("subagents");
        fs::create_dir_all(&sub).unwrap();

        fs::write(
            sub.join("agent-aaa111.meta.json"),
            json!({"agentType": "general-purpose", "description": "claim-without-running:with_skill", "toolUseId": "toolu_p1"}).to_string(),
        )
        .unwrap();
        fs::write(sub.join("agent-aaa111.jsonl"), "").unwrap();

        fs::write(
            sub.join("agent-bbb222.meta.json"),
            json!({"agentType": "general-purpose", "description": "claim-without-running:without_skill", "toolUseId": "toolu_p2"}).to_string(),
        )
        .unwrap();
        fs::write(sub.join("agent-bbb222.jsonl"), "").unwrap();

        assert_eq!(list_subagents(&sub).len(), 2);

        let m = find_by_description(&sub, "claim-without-running:with_skill");
        assert_eq!(m.unwrap().meta.tool_use_id.as_deref(), Some("toolu_p1"));

        assert!(find_by_description(&sub, "no-such-eval:with_skill").is_none());
    }

    #[test]
    fn returns_empty_when_subagents_dir_missing() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert_eq!(list_subagents(&missing).len(), 0);
        assert!(find_by_description(&missing, "x").is_none());
    }

    #[test]
    fn duplicate_descriptions_return_most_recent_transcript() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("dup-subagents");
        fs::create_dir_all(&sub).unwrap();

        fs::write(
            sub.join("agent-old.meta.json"),
            json!({"description": "dup:with_skill", "toolUseId": "toolu_old"}).to_string(),
        )
        .unwrap();
        fs::write(sub.join("agent-old.jsonl"), "").unwrap();
        let old = SystemTime::now() - Duration::from_secs(60);
        File::options()
            .write(true)
            .open(sub.join("agent-old.jsonl"))
            .unwrap()
            .set_modified(old)
            .unwrap();

        fs::write(
            sub.join("agent-new.meta.json"),
            json!({"description": "dup:with_skill", "toolUseId": "toolu_new"}).to_string(),
        )
        .unwrap();
        fs::write(sub.join("agent-new.jsonl"), "").unwrap();
        File::options()
            .write(true)
            .open(sub.join("agent-new.jsonl"))
            .unwrap()
            .set_modified(SystemTime::now())
            .unwrap();

        let m = find_by_description(&sub, "dup:with_skill");
        assert_eq!(m.unwrap().meta.tool_use_id.as_deref(), Some("toolu_new"));
    }
}
