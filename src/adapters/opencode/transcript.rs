//! OpenCode transcript (event-stream) parsing.
//!
//! `opencode run --format json` emits a JSONL event stream whose envelopes
//! carry `{type, timestamp (epoch ms), sessionID, ...data}`. `tool_use` events
//! hold a self-contained tool part — the name at `part.tool`, the arguments at
//! `part.state.input`, and the outcome at `part.state.output` (completed) or
//! `part.state.error` (error) — which become ordered [`ToolInvocation`]s.
//! Token usage is summed from `step_finish` parts (`part.tokens`, cache
//! reads excluded — matching the codex parser's accounting), the final
//! message is the last `text` part's text, and the duration is the spread of
//! the envelope timestamps.

use crate::adapters::TranscriptSummary;
use crate::adapters::transcript::{PermissionDenial, TranscriptEvent, read_jsonl};
use crate::core::ToolInvocation;
use crate::sandbox::GUARD_REASON_PREFIX;
use serde_json::Value;
use std::io;
use std::path::Path;

/// The stable prefix OpenCode's `PermissionDeniedError` always emits before
/// appending the operator's rule list as JSON. The ruleset is operator config,
/// not the refusal explanation, so the parser drops it.
const DENY_RULE_PREFIX: &str =
    "The user has specified a rule which prevents you from using this specific tool call.";
/// Common prefix of `PermissionRejectedError` (headless auto-reject) and
/// `PermissionCorrectedError` (reject with feedback); reached only when an
/// approval ask is not auto-approved, which never happens under the dispatch
/// recipe's `--auto`, but is recognized for robustness.
const REJECT_PREFIX: &str = "The user rejected permission to use this specific tool call";

fn extract_invocations(records: &[Value]) -> Vec<ToolInvocation> {
    let mut invocations = Vec::new();
    for record in records {
        if record.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let Some(part) = record.get("part").and_then(Value::as_object) else {
            continue;
        };
        let Some(name) = part.get("tool").and_then(Value::as_str) else {
            continue;
        };
        let state = part.get("state");
        let args = state
            .and_then(|s| s.get("input"))
            .filter(|input| input.is_object())
            .cloned();
        // Completed states carry `output`, error states carry `error` (both
        // strings in OpenCode's SDK shapes).
        let result = state
            .and_then(|s| s.get("output").or_else(|| s.get("error")))
            .and_then(Value::as_str)
            .map(str::to_string);
        let ordinal = invocations.len() as u32;
        invocations.push(ToolInvocation {
            name: name.to_string(),
            args,
            result: result.map(Value::String),
            ordinal,
        });
    }
    invocations
}

fn extract_events(records: &[Value]) -> Vec<TranscriptEvent> {
    let mut events = Vec::new();
    for record in records {
        let ordinal = events.len() as u32;
        match record.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                let Some(part) = record.get("part").and_then(Value::as_object) else {
                    continue;
                };
                let Some(name) = part.get("tool").and_then(Value::as_str) else {
                    continue;
                };
                let state = part.get("state");
                let args = state
                    .and_then(|s| s.get("input"))
                    .filter(|input| input.is_object())
                    .cloned();
                let result = state
                    .and_then(|s| s.get("output").or_else(|| s.get("error")))
                    .and_then(Value::as_str)
                    .map(|value| Value::String(value.to_string()));
                events.push(TranscriptEvent::ToolInvocation {
                    ordinal,
                    name: name.to_string(),
                    args,
                    result,
                });
            }
            Some("text") => {
                if let Some(text) = record
                    .get("part")
                    .and_then(|part| part.get("text"))
                    .and_then(Value::as_str)
                {
                    events.push(TranscriptEvent::AssistantMessage {
                        ordinal,
                        text: text.to_string(),
                    });
                }
            }
            _ => {}
        }
    }
    events
}

/// Parse an OpenCode `--format json` event stream into ordered tool
/// invocations.
pub fn parse_opencode_events(jsonl_path: &Path) -> io::Result<Vec<ToolInvocation>> {
    Ok(extract_invocations(&read_jsonl::<Value>(jsonl_path)?))
}

/// Parse the tool calls the CLI refused to run from an OpenCode event stream.
///
/// OpenCode has no dedicated refusal channel: a refused call lands as a
/// `tool_use` event with `part.state.status == "error"` whose `error` string
/// OpenCode itself authors at the permission layer (before the tool body
/// runs). An explicit deny rule throws `PermissionDeniedError` with the fixed
/// [`DENY_RULE_PREFIX`]; a headless reject (or reject-with-feedback) throws
/// `PermissionRejectedError`/`PermissionCorrectedError` with [`REJECT_PREFIX`];
/// and the eval write guard throws the shared `eval guard: ` reason. Ordinary
/// tool errors never produce those strings, so matching the prefixes is not
/// guessing from result text. The ruleset JSON and any correction feedback are
/// stripped from the reason; the guard reason is kept verbatim so the pipeline
/// can attribute it via [`GUARD_REASON_PREFIX`]. Records the refused input's
/// *keys*, never its values.
pub fn parse_opencode_permission_denials(jsonl_path: &Path) -> io::Result<Vec<PermissionDenial>> {
    let records = match read_jsonl::<Value>(jsonl_path) {
        Ok(records) => records,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(records
        .iter()
        .filter_map(permission_denial_from_event)
        .collect())
}

fn permission_denial_from_event(record: &Value) -> Option<PermissionDenial> {
    if record.get("type").and_then(Value::as_str) != Some("tool_use") {
        return None;
    }
    let part = record.get("part").and_then(Value::as_object)?;
    let tool = part.get("tool").and_then(Value::as_str)?;
    let state = part.get("state").and_then(Value::as_object)?;
    if state.get("status").and_then(Value::as_str) != Some("error") {
        return None;
    }
    let error = state.get("error").and_then(Value::as_str)?;
    let reason = refusal_reason(error)?;
    let mut input_keys: Vec<String> = state
        .get("input")
        .and_then(Value::as_object)
        .map(|input| input.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    input_keys.sort();
    Some(PermissionDenial {
        tool: tool.to_string(),
        reason: Some(reason),
        input_keys,
    })
}

/// Normalize a tool-error string into the refusal reason, or `None` if it is an
/// ordinary tool error rather than a permission refusal.
fn refusal_reason(error: &str) -> Option<String> {
    // The eval write guard throws the shared reason verbatim — keep it so the
    // pipeline can attribute it (same prefix both sides match against).
    if error.starts_with(GUARD_REASON_PREFIX) {
        return Some(error.to_string());
    }
    // An explicit deny rule appends the operator's rule list as JSON; that is
    // operator config, not the refusal explanation, so only the fixed prefix
    // is kept.
    if error.starts_with(DENY_RULE_PREFIX) {
        return Some(DENY_RULE_PREFIX.to_string());
    }
    // A headless reject, with or without user feedback, normalizes to the same
    // canonical sentence.
    if error.starts_with(REJECT_PREFIX) {
        return Some(format!("{REJECT_PREFIX}."));
    }
    None
}

/// Parse an OpenCode `--format json` event stream into a full
/// [`TranscriptSummary`].
pub fn parse_opencode_events_full(jsonl_path: &Path) -> io::Result<TranscriptSummary> {
    let records = read_jsonl::<Value>(jsonl_path)?;

    let mut first_ts: Option<i64> = None;
    let mut last_ts: Option<i64> = None;
    let mut timestamp_count = 0usize;
    let mut final_text: Option<String> = None;
    let mut total_tokens: Option<i64> = None;
    let session_id = records
        .iter()
        .find_map(|record| record.get("sessionID").and_then(Value::as_str))
        .map(str::to_string);

    for record in &records {
        // OpenCode envelope timestamps are epoch milliseconds (numbers), not
        // RFC 3339 strings.
        if let Some(ts) = record.get("timestamp").and_then(Value::as_i64) {
            if first_ts.is_none() {
                first_ts = Some(ts);
            }
            last_ts = Some(ts);
            timestamp_count += 1;
        }

        let rtype = record.get("type").and_then(Value::as_str);

        if rtype == Some("text")
            && let Some(text) = record
                .get("part")
                .and_then(|p| p.get("text"))
                .and_then(Value::as_str)
        {
            final_text = Some(text.to_string());
        }

        if rtype == Some("step_finish")
            && let Some(tokens) = record
                .get("part")
                .and_then(|p| p.get("tokens"))
                .and_then(Value::as_object)
        {
            let get = |k: &str| tokens.get(k).and_then(Value::as_i64).unwrap_or(0);
            // cache.read/write excluded, matching the codex parser's accounting.
            let sum = get("input") + get("output") + get("reasoning");
            total_tokens = Some(total_tokens.unwrap_or(0) + sum);
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
    use super::{parse_opencode_events, parse_opencode_events_full};
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

    /// A `tool_use` envelope wrapping a tool part with the given state.
    fn tool_use(ts: i64, tool: &str, state: Value) -> Value {
        json!({
            "type": "tool_use",
            "timestamp": ts,
            "sessionID": "ses_1",
            "part": {
                "id": "prt_1",
                "sessionID": "ses_1",
                "messageID": "msg_1",
                "type": "tool",
                "callID": "call_1",
                "tool": tool,
                "state": state,
            }
        })
    }

    #[test]
    fn extracts_completed_and_errored_tool_calls_with_ordinals_args_and_results() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                tool_use(
                    1_000,
                    "bash",
                    json!({"status": "completed", "input": {"command": "bun test", "description": "run tests"}, "output": "2 pass\n0 fail", "title": "bun test", "metadata": {}, "time": {"start": 900, "end": 1_000}}),
                ),
                tool_use(
                    2_000,
                    "edit",
                    json!({"status": "error", "input": {"filePath": "/tmp/x.md", "oldString": "a", "newString": "b"}, "error": "permission denied", "time": {"start": 1_900, "end": 2_000}}),
                ),
                tool_use(
                    3_000,
                    "read",
                    json!({"status": "completed", "output": "file contents", "title": "read", "metadata": {}, "time": {"start": 2_900, "end": 3_000}}),
                ),
            ],
        );

        let result = parse_opencode_events(&path).unwrap();
        assert_eq!(
            result,
            vec![
                ToolInvocation {
                    name: "bash".into(),
                    args: Some(json!({"command": "bun test", "description": "run tests"})),
                    result: Some(Value::String("2 pass\n0 fail".into())),
                    ordinal: 0,
                },
                ToolInvocation {
                    name: "edit".into(),
                    args: Some(
                        json!({"filePath": "/tmp/x.md", "oldString": "a", "newString": "b"})
                    ),
                    result: Some(Value::String("permission denied".into())),
                    ordinal: 1,
                },
                // No `input` object on the state → null args.
                ToolInvocation {
                    name: "read".into(),
                    args: None,
                    result: Some(Value::String("file contents".into())),
                    ordinal: 2,
                },
            ]
        );
    }

    #[test]
    fn skill_tool_calls_keep_the_name_argument_for_the_meta_check() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[tool_use(
                1_000,
                "skill",
                json!({"status": "completed", "input": {"name": "slow-powers-eval-1-with-skill-mr-review"}, "output": "<skill_content/>", "title": "skill", "metadata": {}, "time": {"start": 900, "end": 1_000}}),
            )],
        );

        let invocations = parse_opencode_events(&path).unwrap();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].name, "skill");
        assert_eq!(
            invocations[0].args.as_ref().and_then(|a| a.get("name")),
            Some(&json!("slow-powers-eval-1-with-skill-mr-review"))
        );
    }

    #[test]
    fn non_tool_events_produce_no_invocations() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "step_start", "timestamp": 1_000, "sessionID": "ses_1", "part": {"id": "p1", "type": "step-start"}}),
                json!({"type": "reasoning", "timestamp": 2_000, "sessionID": "ses_1", "part": {"id": "p2", "type": "reasoning", "text": "thinking"}}),
                json!({"type": "text", "timestamp": 3_000, "sessionID": "ses_1", "part": {"id": "p3", "type": "text", "text": "hello"}}),
                json!({"type": "step_finish", "timestamp": 4_000, "sessionID": "ses_1", "part": {"id": "p4", "type": "step-finish", "reason": "stop", "cost": 0.0, "tokens": {"input": 1, "output": 1, "reasoning": 0, "cache": {"read": 0, "write": 0}}}}),
                json!({"type": "error", "timestamp": 5_000, "sessionID": "ses_1", "error": {"name": "UnknownError", "data": {"message": "boom"}}}),
            ],
        );
        assert_eq!(parse_opencode_events(&path).unwrap(), vec![]);
    }

    #[test]
    fn malformed_tool_use_parts_are_skipped() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "tool_use", "timestamp": 1_000, "sessionID": "ses_1"}),
                json!({"type": "tool_use", "timestamp": 2_000, "sessionID": "ses_1", "part": "not an object"}),
                json!({"type": "tool_use", "timestamp": 3_000, "sessionID": "ses_1", "part": {"id": "p1", "type": "tool"}}),
                json!({"type": "tool_use", "timestamp": 4_000, "sessionID": "ses_1", "part": {"id": "p2", "type": "tool", "tool": 7}}),
            ],
        );
        assert_eq!(parse_opencode_events(&path).unwrap(), vec![]);
    }

    #[test]
    fn skips_malformed_jsonl_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        let good = tool_use(
            1_000,
            "grep",
            json!({"status": "completed", "input": {"pattern": "todo"}, "output": "3 matches", "title": "grep", "metadata": {}, "time": {"start": 900, "end": 1_000}}),
        );
        fs::write(&path, format!("{good}\nnot valid json\n")).unwrap();
        assert_eq!(parse_opencode_events(&path).unwrap().len(), 1);
    }

    #[test]
    fn full_summary_extracts_final_text_token_sum_and_timestamp_duration() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "step_start", "timestamp": 1_000, "sessionID": "ses_1", "part": {"id": "p1", "type": "step-start"}}),
                tool_use(
                    2_000,
                    "bash",
                    json!({"status": "completed", "input": {"command": "ls"}, "output": "README.md", "title": "ls", "metadata": {}, "time": {"start": 1_900, "end": 2_000}}),
                ),
                json!({"type": "text", "timestamp": 3_000, "sessionID": "ses_1", "part": {"id": "p3", "type": "text", "text": "First."}}),
                json!({"type": "text", "timestamp": 4_000, "sessionID": "ses_1", "part": {"id": "p4", "type": "text", "text": "Final."}}),
                json!({"type": "step_finish", "timestamp": 5_000, "sessionID": "ses_1", "part": {"id": "p5", "type": "step-finish", "reason": "stop", "cost": 0.002, "tokens": {"input": 100, "output": 20, "reasoning": 5, "cache": {"read": 75, "write": 0}}}}),
                json!({"type": "step_finish", "timestamp": 6_000, "sessionID": "ses_1", "part": {"id": "p6", "type": "step-finish", "reason": "stop", "cost": 0.001, "tokens": {"input": 40, "output": 2, "reasoning": 0, "cache": {"read": 0, "write": 0}}}}),
            ],
        );

        let full = parse_opencode_events_full(&path).unwrap();
        assert_eq!(full.session_id.as_deref(), Some("ses_1"));
        assert_eq!(
            serde_json::to_value(&full.events).unwrap(),
            json!([
                {
                    "type": "tool_invocation",
                    "ordinal": 0,
                    "name": "bash",
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
        assert_eq!(full.tool_invocations.len(), 1);
        assert_eq!(full.tool_invocations[0].name, "bash");
        assert_eq!(full.final_text, Some("Final.".into()));
        // cache.read (75) excluded, matching the codex parser's accounting.
        assert_eq!(full.total_tokens, Some(167));
        assert_eq!(full.duration_ms, Some(5_000));
    }

    #[test]
    fn returns_null_usage_and_duration_when_sparse() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "text", "timestamp": 1_000, "sessionID": "ses_1", "part": {"id": "p1", "type": "text", "text": "Done."}}),
            ],
        );
        let full = parse_opencode_events_full(&path).unwrap();
        assert_eq!(full.final_text, Some("Done.".into()));
        assert_eq!(full.total_tokens, None);
        assert_eq!(full.duration_ms, None);
    }

    #[test]
    fn step_finish_without_tokens_leaves_the_total_untouched() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "step_finish", "timestamp": 1_000, "sessionID": "ses_1", "part": {"id": "p1", "type": "step-finish", "reason": "tool-calls", "cost": 0.0}}),
                json!({"type": "step_finish", "timestamp": 2_000, "sessionID": "ses_1", "part": {"id": "p2", "type": "step-finish", "reason": "stop", "cost": 0.001, "tokens": {"input": 10, "output": 2, "reasoning": 0, "cache": {"read": 0, "write": 0}}}}),
            ],
        );
        assert_eq!(
            parse_opencode_events_full(&path).unwrap().total_tokens,
            Some(12)
        );
    }
}
