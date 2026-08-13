//! Cline transcript (event-stream) parsing.
//!
//! `cline --json` emits NDJSON: `agent_event` wrappers (`content_start` /
//! `content_end` / `usage` / `iteration_start` / `iteration_end` / `done`),
//! `hook_event` lifecycle lines, and a terminal `run_result` carrying the final
//! text, run token usage, and `durationMs`. Tool calls arrive as a
//! `content_start` event (self-contained: `toolCallId`, `toolName`, and args
//! nested under `input`) paired with a `content_end` event (same `toolCallId`)
//! whose `output` is the tool's result payload. Verified against cline 3.0.53
//! with a live dispatch capture; per-shape evidence lives in
//! `docs/cline-notes.md`.

use std::collections::HashMap;
use std::io;
use std::path::Path;

use serde_json::Value;

use crate::adapters::TranscriptSummary;
use crate::adapters::transcript::{PermissionDenial, TranscriptEvent, read_jsonl};
use crate::core::ToolInvocation;
use crate::sandbox::GUARD_REASON_PREFIX;

/// Flatten a `content_start` tool event's nested `input` into the canonical
/// top-level arg shape the pipeline consumes. Everything hoists verbatim
/// except `run_commands`' `commands` array, which joins into the single
/// `command` string the stray-writes audit and the write guard classify.
fn flatten_args(tool: &str, input: &Value) -> Option<Value> {
    let obj = input.as_object()?;
    if tool == "run_commands" {
        let commands = obj
            .get("commands")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let mut flat = obj.clone();
        flat.remove("commands");
        flat.insert("command".to_string(), Value::String(commands));
        return Some(Value::Object(flat));
    }
    Some(Value::Object(obj.clone()))
}

/// Coerce a `content_end` tool event's `output` into result text. Observed
/// shapes (3.0.53): `skills` returns a bare string, `editor` a single
/// `{query, result, success}` object, `run_commands`/`read_files` arrays of
/// those objects (one entry per command/file), and a refused/blocked call an
/// `{"error": "<reason>"}` object.
fn coerce_result(output: &Value) -> Option<Value> {
    match output {
        Value::String(text) => Some(Value::String(text.clone())),
        Value::Object(obj) => obj
            .get("result")
            .or_else(|| obj.get("error"))
            .and_then(Value::as_str)
            .map(|text| Value::String(text.to_string())),
        Value::Array(items) => {
            let parts: Vec<&str> = items
                .iter()
                .filter_map(|item| item.get("result").and_then(Value::as_str))
                .collect();
            (!parts.is_empty()).then(|| Value::String(parts.join("\n")))
        }
        _ => None,
    }
}

/// One stream-ordered parse item: a tool call (index into the invocation
/// list, result attached when its `content_end` arrives) or a complete
/// assistant text block.
enum Item {
    Tool(usize),
    Text(String),
}

/// Walk the `agent_event` records once, collecting tool invocations (paired
/// by `toolCallId`) and complete assistant text blocks in stream order.
fn parse_items(records: &[Value]) -> (Vec<ToolInvocation>, Vec<Item>) {
    let mut invocations: Vec<ToolInvocation> = Vec::new();
    let mut items: Vec<Item> = Vec::new();
    let mut open: HashMap<String, usize> = HashMap::new();
    for record in records {
        if record.get("type").and_then(Value::as_str) != Some("agent_event") {
            continue;
        }
        let Some(event) = record.get("event").and_then(Value::as_object) else {
            continue;
        };
        match (
            event.get("type").and_then(Value::as_str),
            event.get("contentType").and_then(Value::as_str),
        ) {
            (Some("content_start"), Some("tool")) => {
                let (Some(id), Some(name)) = (
                    event.get("toolCallId").and_then(Value::as_str),
                    event.get("toolName").and_then(Value::as_str),
                ) else {
                    continue;
                };
                let args = event
                    .get("input")
                    .and_then(|input| flatten_args(name, input));
                open.insert(id.to_string(), invocations.len());
                items.push(Item::Tool(invocations.len()));
                invocations.push(ToolInvocation {
                    name: name.to_string(),
                    args,
                    result: None,
                    ordinal: invocations.len() as u32,
                });
            }
            (Some("content_end"), Some("tool")) => {
                let Some(id) = event.get("toolCallId").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(&index) = open.get(id)
                    && let Some(output) = event.get("output")
                {
                    invocations[index].result = coerce_result(output);
                }
            }
            // `content_end` text blocks are complete; the `content_start`
            // text/reasoning chunks are streaming partials and never surface.
            (Some("content_end"), Some("text")) => {
                if let Some(text) = event.get("text").and_then(Value::as_str) {
                    items.push(Item::Text(text.to_string()));
                }
            }
            _ => {}
        }
    }
    (invocations, items)
}

fn extract_invocations(records: &[Value]) -> Vec<ToolInvocation> {
    parse_items(records).0
}

fn extract_events(invocations: &[ToolInvocation], items: &[Item]) -> Vec<TranscriptEvent> {
    let mut events = Vec::new();
    for item in items {
        let ordinal = events.len() as u32;
        match item {
            Item::Tool(index) => {
                let inv = &invocations[*index];
                events.push(TranscriptEvent::ToolInvocation {
                    ordinal,
                    name: inv.name.clone(),
                    args: inv.args.clone(),
                    result: inv.result.clone(),
                });
            }
            Item::Text(text) => events.push(TranscriptEvent::AssistantMessage {
                ordinal,
                text: text.clone(),
            }),
        }
    }
    events
}

/// Parse a `cline --json` event stream into ordered tool invocations.
pub fn parse_cline_events(jsonl_path: &Path) -> io::Result<Vec<ToolInvocation>> {
    Ok(extract_invocations(&read_jsonl::<Value>(jsonl_path)?))
}

/// True when a `content_end` error string is a permission refusal rather than
/// an ordinary tool error. The refusal wordings are the runtime's own
/// fixed strings (verified in the 3.0.53 binary): the policy gate's
/// `Tool "<name>" is disabled by policy` / `Tool "<name>" was not approved`
/// and the hook gate's `Tool <name> was blocked by a runtime hook`. The eval
/// write guard's shared `eval guard: ` reason is kept verbatim so the
/// pipeline can attribute it via [`GUARD_REASON_PREFIX`].
fn refusal_reason(error: &str) -> Option<String> {
    if error.starts_with(GUARD_REASON_PREFIX) {
        return Some(error.to_string());
    }
    let policy_refusal = error.starts_with("Tool ")
        && (error.ends_with("is disabled by policy")
            || error.ends_with("was not approved")
            || error.ends_with("was blocked by a runtime hook"));
    policy_refusal.then(|| error.to_string())
}

/// Parse the tool calls the CLI refused to run from a Cline event stream.
///
/// A refused call never executes, so its `content_end` carries
/// `{"error": "<reason>"}` instead of a result payload (observed in the
/// 3.0.53 guard-plugin spike; the same shape carries the policy gate's
/// refusals). The refused input's *keys* come from the paired
/// `content_start` — values never surface. A missing events file yields an
/// empty vec (no evidence), matching the other readers.
pub fn parse_cline_permission_denials(jsonl_path: &Path) -> io::Result<Vec<PermissionDenial>> {
    let records = match read_jsonl::<Value>(jsonl_path) {
        Ok(records) => records,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    // Index each call's start record so the denial can name the tool and its
    // input keys; the `content_end` carries neither.
    let mut starts: HashMap<String, (String, Vec<String>)> = HashMap::new();
    let mut denials = Vec::new();
    for record in &records {
        if record.get("type").and_then(Value::as_str) != Some("agent_event") {
            continue;
        }
        let Some(event) = record.get("event").and_then(Value::as_object) else {
            continue;
        };
        if event.get("contentType").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let Some(id) = event.get("toolCallId").and_then(Value::as_str) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("content_start") => {
                let Some(name) = event.get("toolName").and_then(Value::as_str) else {
                    continue;
                };
                let mut keys: Vec<String> = event
                    .get("input")
                    .and_then(Value::as_object)
                    .map(|input| input.keys().cloned().collect())
                    .unwrap_or_default();
                keys.sort();
                starts.insert(id.to_string(), (name.to_string(), keys));
            }
            Some("content_end") => {
                let Some(error) = event
                    .get("output")
                    .and_then(Value::as_object)
                    .and_then(|output| output.get("error"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let Some(reason) = refusal_reason(error) else {
                    continue;
                };
                let Some((tool, input_keys)) = starts.get(id) else {
                    continue;
                };
                denials.push(PermissionDenial {
                    tool: tool.clone(),
                    reason: Some(reason),
                    input_keys: input_keys.clone(),
                });
            }
            _ => {}
        }
    }
    Ok(denials)
}

/// Parse a `cline --json` event stream into a full [`TranscriptSummary`].
///
/// The terminal `run_result` line carries the final text, the run's token
/// usage (cache reads subtracted, matching the codex parser's accounting),
/// and `durationMs`. There is no session id anywhere in the stream, so
/// `session_id` is always `None`.
pub fn parse_cline_events_full(jsonl_path: &Path) -> io::Result<TranscriptSummary> {
    let records = read_jsonl::<Value>(jsonl_path)?;
    let (invocations, items) = parse_items(&records);

    let mut final_text = None;
    let mut total_tokens = None;
    let mut duration_ms = None;
    for record in &records {
        if record.get("type").and_then(Value::as_str) != Some("run_result") {
            continue;
        }
        if let Some(text) = record.get("text").and_then(Value::as_str) {
            final_text = Some(text.to_string());
        }
        if let Some(usage) = record.get("usage").and_then(Value::as_object) {
            let get = |k: &str| usage.get(k).and_then(Value::as_i64).unwrap_or(0);
            total_tokens = Some(get("inputTokens") + get("outputTokens") - get("cacheReadTokens"));
        }
        if let Some(ms) = record.get("durationMs").and_then(Value::as_i64) {
            duration_ms = Some(ms);
        }
    }

    Ok(TranscriptSummary {
        events: extract_events(&invocations, &items),
        tool_invocations: invocations,
        session_id: None,
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

    use super::{parse_cline_events, parse_cline_events_full, parse_cline_permission_denials};
    use crate::adapters::PermissionDenial;
    use crate::core::ToolInvocation;

    fn write_jsonl(path: &Path, lines: &[Value]) {
        let body = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{body}\n")).unwrap();
    }

    fn content_start(id: &str, tool: &str, input: Value) -> Value {
        json!({"ts": "2026-08-13T03:00:00.000Z", "type": "agent_event", "event": {
            "type": "content_start", "contentType": "tool",
            "toolCallId": id, "toolName": tool, "input": input,
        }})
    }

    fn content_end(id: &str, tool: &str, output: Value) -> Value {
        json!({"ts": "2026-08-13T03:00:01.000Z", "type": "agent_event", "event": {
            "type": "content_end", "contentType": "tool",
            "toolCallId": id, "toolName": tool, "output": output,
        }})
    }

    /// The four tool shapes observed in the 3.0.53 capture: `skills` output is
    /// a bare string, `editor`'s a single `{query, result, success}` object,
    /// and `run_commands`/`read_files` outputs arrays of those objects.
    #[test]
    fn extracts_invocations_with_flattened_args_and_coerced_results() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                content_start("skills_0", "skills", json!({"skill": "probe_skill"})),
                content_end("skills_0", "skills", json!("PROBE_SKILL_LOADED")),
                content_start(
                    "editor_1",
                    "editor",
                    json!({"path": "/x/out.txt", "new_text": "hello"}),
                ),
                content_end(
                    "editor_1",
                    "editor",
                    json!({"query": "edit:/x/out.txt", "result": "File created successfully at: /x/out.txt", "success": true}),
                ),
                content_start(
                    "run_commands_2",
                    "run_commands",
                    json!({"commands": ["cat out.txt"]}),
                ),
                content_end(
                    "run_commands_2",
                    "run_commands",
                    json!([{"query": "cat out.txt", "result": "hello", "success": true}]),
                ),
                content_start(
                    "read_files_3",
                    "read_files",
                    json!({"files": [{"path": "/x/seed.txt"}]}),
                ),
                content_end(
                    "read_files_3",
                    "read_files",
                    json!([{"query": "/x/seed.txt", "result": "1 | seed", "success": true}]),
                ),
            ],
        );

        let invocations = parse_cline_events(&path).unwrap();
        assert_eq!(
            invocations,
            vec![
                ToolInvocation {
                    name: "skills".into(),
                    args: Some(json!({"skill": "probe_skill"})),
                    result: Some(Value::String("PROBE_SKILL_LOADED".into())),
                    ordinal: 0,
                },
                ToolInvocation {
                    name: "editor".into(),
                    args: Some(json!({"path": "/x/out.txt", "new_text": "hello"})),
                    result: Some(Value::String(
                        "File created successfully at: /x/out.txt".into()
                    )),
                    ordinal: 1,
                },
                // The commands array joins into one `command` string so the
                // stray-writes audit and guard can classify it.
                ToolInvocation {
                    name: "run_commands".into(),
                    args: Some(json!({"command": "cat out.txt"})),
                    result: Some(Value::String("hello".into())),
                    ordinal: 2,
                },
                ToolInvocation {
                    name: "read_files".into(),
                    args: Some(json!({"files": [{"path": "/x/seed.txt"}]})),
                    result: Some(Value::String("1 | seed".into())),
                    ordinal: 3,
                },
            ]
        );
    }

    /// Stream order mixes tool calls and assistant text: the events list
    /// interleaves them under one ordinal counter, and the summary reads the
    /// terminal `run_result` (cache reads subtracted, matching codex
    /// accounting). Streaming `content_start` text chunks are partials and
    /// never surface.
    #[test]
    fn full_summary_reads_run_result_and_interleaves_events() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"ts": "2026-08-13T02:59:00.000Z", "type": "agent_event", "event": {"type": "content_start", "contentType": "text", "text": "I'll"}}),
                json!({"ts": "2026-08-13T02:59:00.100Z", "type": "agent_event", "event": {"type": "content_start", "contentType": "text", "text": " create"}}),
                content_start(
                    "editor_0",
                    "editor",
                    json!({"path": "/x/out.txt", "new_text": "hi"}),
                ),
                json!({"ts": "2026-08-13T02:59:02.000Z", "type": "agent_event", "event": {"type": "content_end", "contentType": "text", "text": "I'll create it."}}),
                content_end(
                    "editor_0",
                    "editor",
                    json!({"query": "edit:/x/out.txt", "result": "File created successfully at: /x/out.txt", "success": true}),
                ),
                json!({"ts": "2026-08-13T02:59:04.000Z", "type": "agent_event", "event": {"type": "content_end", "contentType": "text", "text": "Done."}}),
                json!({"ts": "2026-08-13T02:59:05.000Z", "type": "hook_event", "hookEventName": "agent_end", "agentId": "agent_1", "taskId": "conv_1", "parentAgentId": null}),
                json!({"ts": "2026-08-13T02:59:06.000Z", "type": "run_result", "finishReason": "completed", "iterations": 2, "usage": {"inputTokens": 100, "outputTokens": 20, "cacheReadTokens": 40, "cacheWriteTokens": 0, "totalCost": 0.001}, "aggregateUsage": {"inputTokens": 100, "outputTokens": 20, "cacheReadTokens": 40, "cacheWriteTokens": 0, "totalCost": 0.001}, "durationMs": 12345, "text": "Done."}),
            ],
        );

        let full = parse_cline_events_full(&path).unwrap();
        assert_eq!(full.session_id, None);
        assert_eq!(full.total_tokens, Some(80));
        assert_eq!(full.duration_ms, Some(12_345));
        assert_eq!(full.final_text.as_deref(), Some("Done."));
        assert_eq!(full.tool_invocations.len(), 1);
        assert_eq!(
            serde_json::to_value(&full.events).unwrap(),
            json!([
                {
                    "type": "tool_invocation",
                    "ordinal": 0,
                    "name": "editor",
                    "args": {"path": "/x/out.txt", "new_text": "hi"},
                    "result": "File created successfully at: /x/out.txt",
                },
                {"type": "assistant_message", "ordinal": 1, "text": "I'll create it."},
                {"type": "assistant_message", "ordinal": 2, "text": "Done."},
            ])
        );
    }

    /// No `run_result` (stream cut short) leaves tokens/duration/final text
    /// unset; an unpaired `content_start` keeps its invocation with no result,
    /// and a multi-entry `commands` array joins with newlines.
    #[test]
    fn sparse_stream_yields_null_usage_and_unpaired_call() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                content_start(
                    "run_commands_0",
                    "run_commands",
                    json!({"commands": ["ls", "pwd"]}),
                ),
                json!({"ts": "2026-08-13T02:59:04.000Z", "type": "agent_event", "event": {"type": "content_end", "contentType": "text", "text": "partial"}}),
            ],
        );

        let full = parse_cline_events_full(&path).unwrap();
        assert_eq!(full.total_tokens, None);
        assert_eq!(full.duration_ms, None);
        assert_eq!(full.final_text, None);
        assert_eq!(full.tool_invocations.len(), 1);
        assert_eq!(
            full.tool_invocations[0].args,
            Some(json!({"command": "ls\npwd"}))
        );
        assert_eq!(full.tool_invocations[0].result, None);
    }

    /// A refused call lands as a `content_end` whose output is `{"error":
    /// "<reason>"}` (3.0.53 spike capture: a `beforeTool` plugin block, the
    /// policy gate's `disabled by policy` / `was not approved`). Only those
    /// refusal markers count — ordinary tool errors never do — and the guard
    /// reason stays verbatim so the pipeline can attribute it. Input *keys*
    /// come from the paired `content_start`; values never surface.
    #[test]
    fn permission_denials_recognize_refusals_and_skip_ordinary_errors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                content_start(
                    "editor_0",
                    "editor",
                    json!({"path": "/etc/passwd", "new_text": "x"}),
                ),
                json!({"ts": "2026-08-13T03:00:01.000Z", "type": "agent_event", "event": {"type": "content_end", "contentType": "tool", "toolCallId": "editor_0", "toolName": "editor", "output": {"error": "eval guard: editor to /etc/passwd is outside the eval sandbox (allowed: /work/.eval-magic)"}}}),
                content_start(
                    "run_commands_1",
                    "run_commands",
                    json!({"commands": ["rm -rf /"]}),
                ),
                json!({"ts": "2026-08-13T03:00:02.000Z", "type": "agent_event", "event": {"type": "content_end", "contentType": "tool", "toolCallId": "run_commands_1", "toolName": "run_commands", "output": {"error": "Tool \"run_commands\" is disabled by policy"}}}),
                content_start("editor_2", "editor", json!({"path": "/x/y" })),
                json!({"ts": "2026-08-13T03:00:03.000Z", "type": "agent_event", "event": {"type": "content_end", "contentType": "tool", "toolCallId": "editor_2", "toolName": "editor", "output": {"error": "Tool \"editor\" was not approved"}}}),
                content_start("editor_3", "editor", json!({"path": "/x/z"})),
                json!({"ts": "2026-08-13T03:00:04.000Z", "type": "agent_event", "event": {"type": "content_end", "contentType": "tool", "toolCallId": "editor_3", "toolName": "editor", "output": {"error": "replacement text not found in file"}}}),
                content_start(
                    "read_files_4",
                    "read_files",
                    json!({"files": [{"path": "/x/a"}]}),
                ),
                content_end(
                    "read_files_4",
                    "read_files",
                    json!([{"query": "/x/a", "result": "contents", "success": true}]),
                ),
            ],
        );

        let denials = parse_cline_permission_denials(&path).unwrap();
        assert_eq!(
            denials,
            vec![
                PermissionDenial {
                    tool: "editor".into(),
                    reason: Some(
                        "eval guard: editor to /etc/passwd is outside the eval sandbox (allowed: /work/.eval-magic)".into()
                    ),
                    input_keys: vec!["new_text".into(), "path".into()],
                },
                PermissionDenial {
                    tool: "run_commands".into(),
                    reason: Some("Tool \"run_commands\" is disabled by policy".into()),
                    input_keys: vec!["commands".into()],
                },
                PermissionDenial {
                    tool: "editor".into(),
                    reason: Some("Tool \"editor\" was not approved".into()),
                    input_keys: vec!["path".into()],
                },
            ]
        );
    }

    /// The error payload of a refused call still surfaces as the invocation's
    /// result text, so transcripts show why the call never ran.
    #[test]
    fn error_output_coerces_to_result_text() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                content_start(
                    "editor_0",
                    "editor",
                    json!({"path": "/etc/passwd", "new_text": "x"}),
                ),
                json!({"ts": "2026-08-13T03:00:01.000Z", "type": "agent_event", "event": {"type": "content_end", "contentType": "tool", "toolCallId": "editor_0", "toolName": "editor", "output": {"error": "eval guard: blocked"}}}),
            ],
        );

        let invocations = parse_cline_events(&path).unwrap();
        assert_eq!(
            invocations[0].result,
            Some(Value::String("eval guard: blocked".into()))
        );
    }
}
