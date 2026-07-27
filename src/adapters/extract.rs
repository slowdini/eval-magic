//! Declarative transcript extraction for flat event streams.
//!
//! [`ExtractSpec`] is the data half of a descriptor's `[transcript.extract]`
//! block: equality filters, field picks, a flat tool-item mapping, a token
//! reduction, and a duration rule interpreted by this one generic engine. A
//! stream that needs more (keyed cross-event joins, content coercion) is a named code capability
//! ([`super::capabilities::TranscriptParser`]), not a bigger spec.
//!
//! Fixed rules the spec cannot change: `final_text` and `duration.field` take
//! the last match; `timestamp_spread` needs at least two parseable RFC 3339
//! timestamps; `result_coalesce` takes the first *present* field
//! (present-but-null counts), strings verbatim and everything else as compact
//! JSON; args preserve the item's key order; dotted paths descend objects
//! only, so keys containing literal dots are unaddressable.

use crate::adapters::TranscriptSummary;
use crate::adapters::transcript::{TranscriptEvent, read_jsonl};
use crate::core::ToolInvocation;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// Dotted-path → expected-string equality filter; empty matches every record.
type Where = BTreeMap<String, String>;

/// The `[transcript.extract]` block: which normalized outputs to produce and
/// how. Validation requires at least one sub-table.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtractSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsExtract>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_text: Option<FieldPick>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_messages: Option<FieldPick>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<FieldPick>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokensExtract>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<DurationExtract>,
}

/// Flat tool-item mapping: each matching record's item object becomes one
/// [`ToolInvocation`].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsExtract {
    #[serde(default, rename = "where", skip_serializing_if = "BTreeMap::is_empty")]
    pub r#where: Where,
    /// Dotted path to the tool object within the record; omit when the record
    /// itself is the item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// Field of the item whose string value is the invocation name; records
    /// without it are skipped.
    pub name_field: String,
    /// Names that are not tools (e.g. message/reasoning items).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip_names: Vec<String>,
    /// Structural keys excluded from args; whatever remains is the args
    /// object, `null` when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args_omit: Vec<String>,
    /// Result = first present of these item fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub result_coalesce: Vec<String>,
}

/// Field pick over matching records; the last match wins.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FieldPick {
    #[serde(default, rename = "where", skip_serializing_if = "BTreeMap::is_empty")]
    pub r#where: Where,
    pub field: String,
}

/// Token reduction: over matching records, sum the listed integer fields, then
/// subtract the optional listed integer fields and clamp each record at zero. A
/// record where no `sum` path resolves leaves the total untouched (it never
/// turns an absent total into zero).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TokensExtract {
    #[serde(default, rename = "where", skip_serializing_if = "BTreeMap::is_empty")]
    pub r#where: Where,
    pub sum: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtract: Vec<String>,
}

/// Duration: either a direct millisecond field pick (last match wins) or the
/// spread between the first and last RFC 3339 timestamps. Validation requires
/// exactly one variant.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DurationExtract {
    #[serde(default, rename = "where", skip_serializing_if = "BTreeMap::is_empty")]
    pub r#where: Where,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_spread: Option<String>,
}

/// Extract ordered tool invocations from a JSONL events file.
pub(crate) fn parse(spec: &ExtractSpec, path: &Path) -> io::Result<Vec<ToolInvocation>> {
    let records = read_jsonl::<Value>(path)?;
    Ok(extract_tools(spec, &records))
}

/// Extract a full [`TranscriptSummary`] from a JSONL events file.
pub(crate) fn parse_full(spec: &ExtractSpec, path: &Path) -> io::Result<TranscriptSummary> {
    let records = read_jsonl::<Value>(path)?;
    Ok(TranscriptSummary {
        tool_invocations: extract_tools(spec, &records),
        events: extract_events(spec, &records),
        session_id: spec
            .session_id
            .as_ref()
            .and_then(|pick| extract_final_text(pick, &records)),
        total_tokens: spec
            .tokens
            .as_ref()
            .and_then(|t| extract_tokens(t, &records)),
        duration_ms: spec
            .duration
            .as_ref()
            .and_then(|d| extract_duration(d, &records)),
        final_text: spec
            .final_text
            .as_ref()
            .and_then(|f| extract_final_text(f, &records)),
    })
}

/// Follow a dotted path through nested objects.
fn resolve<'a>(record: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(record, |value, segment| value.get(segment))
}

fn matches(record: &Value, filter: &Where) -> bool {
    filter
        .iter()
        .all(|(path, expected)| resolve(record, path).and_then(Value::as_str) == Some(expected))
}

fn stringify_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn extract_tools(spec: &ExtractSpec, records: &[Value]) -> Vec<ToolInvocation> {
    let mut invocations = Vec::new();
    for record in records {
        let ordinal = invocations.len() as u32;
        if let Some(invocation) = extract_tool(spec.tools.as_ref(), record, ordinal) {
            invocations.push(invocation);
        }
    }
    invocations
}

fn extract_tool(
    tools: Option<&ToolsExtract>,
    record: &Value,
    ordinal: u32,
) -> Option<ToolInvocation> {
    let tools = tools?;
    if !matches(record, &tools.r#where) {
        return None;
    }
    let item_value = match &tools.item {
        Some(path) => resolve(record, path)?,
        None => record,
    };
    let item = item_value.as_object()?;
    let name = item.get(&tools.name_field).and_then(Value::as_str)?;
    if tools.skip_names.iter().any(|skip| skip == name) {
        return None;
    }
    let mut args = serde_json::Map::new();
    for (key, value) in item {
        if tools.args_omit.iter().any(|omit| omit == key) {
            continue;
        }
        args.insert(key.clone(), value.clone());
    }
    let result = tools
        .result_coalesce
        .iter()
        .find_map(|key| item.get(key).map(stringify_value));
    Some(ToolInvocation {
        name: name.to_string(),
        args: (!args.is_empty()).then_some(Value::Object(args)),
        result: result.map(Value::String),
        ordinal,
    })
}

fn extract_events(spec: &ExtractSpec, records: &[Value]) -> Vec<TranscriptEvent> {
    let mut events = Vec::new();
    for record in records {
        if let Some(pick) = &spec.assistant_messages
            && matches(record, &pick.r#where)
            && let Some(text) = resolve(record, &pick.field).and_then(Value::as_str)
        {
            events.push(TranscriptEvent::AssistantMessage {
                ordinal: events.len() as u32,
                text: text.to_string(),
            });
        }
        if let Some(invocation) = extract_tool(spec.tools.as_ref(), record, events.len() as u32) {
            events.push(TranscriptEvent::ToolInvocation {
                ordinal: invocation.ordinal,
                name: invocation.name,
                args: invocation.args,
                result: invocation.result,
            });
        }
    }
    events
}

fn extract_final_text(pick: &FieldPick, records: &[Value]) -> Option<String> {
    records
        .iter()
        .filter(|record| matches(record, &pick.r#where))
        .filter_map(|record| resolve(record, &pick.field).and_then(Value::as_str))
        .next_back()
        .map(str::to_string)
}

fn extract_tokens(tokens: &TokensExtract, records: &[Value]) -> Option<i64> {
    let mut total: Option<i64> = None;
    for record in records {
        if !matches(record, &tokens.r#where) {
            continue;
        }
        let resolved: Vec<&Value> = tokens
            .sum
            .iter()
            .filter_map(|path| resolve(record, path))
            .collect();
        // No listed path resolves: the record carries no token report, and
        // must not turn an absent total into zero.
        if resolved.is_empty() {
            continue;
        }
        let sum: i64 = resolved.iter().filter_map(|v| v.as_i64()).sum();
        let subtract: i64 = tokens
            .subtract
            .iter()
            .filter_map(|path| resolve(record, path).and_then(Value::as_i64))
            .sum();
        let subtotal = sum.saturating_sub(subtract).max(0);
        total = Some(total.unwrap_or(0) + subtotal);
    }
    total
}

fn parse_millis(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn extract_duration(duration: &DurationExtract, records: &[Value]) -> Option<i64> {
    let matching = records
        .iter()
        .filter(|record| matches(record, &duration.r#where));
    if let Some(field) = &duration.field {
        return matching
            .filter_map(|record| resolve(record, field).and_then(Value::as_i64))
            .next_back();
    }
    let ts_field = duration.timestamp_spread.as_ref()?;
    let mut first: Option<i64> = None;
    let mut last: Option<i64> = None;
    let mut count = 0usize;
    for record in matching {
        let Some(ts) = resolve(record, ts_field)
            .and_then(Value::as_str)
            .and_then(parse_millis)
        else {
            continue;
        };
        if first.is_none() {
            first = Some(ts);
        }
        last = Some(ts);
        count += 1;
    }
    match (first, last) {
        (Some(f), Some(l)) if count >= 2 => Some(l - f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn write_jsonl(path: &Path, lines: &[Value]) {
        let body = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{body}\n")).unwrap();
    }

    /// The Codex stream expressed in the extractor language — the worked
    /// example from the BYOH guide, kept equivalent to the `codex-items` code
    /// parser by the differential test below.
    fn codex_spec() -> ExtractSpec {
        toml::from_str(
            r#"
            [tools]
            where = { type = "item.completed" }
            item = "item"
            name_field = "type"
            skip_names = ["agent_message", "reasoning", "plan_update"]
            args_omit = ["id", "type", "status", "output", "result", "error"]
            result_coalesce = ["output", "result", "error"]

            [final_text]
            where = { type = "item.completed", "item.type" = "agent_message" }
            field = "item.text"

            [assistant_messages]
            where = { type = "item.completed", "item.type" = "agent_message" }
            field = "item.text"

            [session_id]
            where = { type = "thread.started" }
            field = "thread_id"

            [tokens]
            where = { type = "turn.completed" }
            sum = ["usage.input_tokens", "usage.output_tokens"]
            subtract = ["usage.cached_input_tokens"]

            [duration]
            timestamp_spread = "timestamp"
            "#,
        )
        .unwrap()
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

        let result = parse(&codex_spec(), &path).unwrap();
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
            parse(&codex_spec(), &path).unwrap(),
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
            parse(&codex_spec(), &path).unwrap(),
            vec![ToolInvocation {
                name: "web_search".into(),
                args: Some(json!({"query": "codex events", "text": "search summary"})),
                result: None,
                ordinal: 0,
            }]
        );
    }

    #[test]
    fn does_not_treat_skip_named_items_as_tools() {
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
        assert_eq!(parse(&codex_spec(), &path).unwrap(), vec![]);
    }

    #[test]
    fn extracts_invocations_last_agent_text_usage_and_duration() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("full.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "thread.started", "thread_id": "thread-flat-1", "timestamp": "2026-06-07T10:00:00.000Z"}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:03.000Z", "item": {"id": "item_1", "type": "command_execution", "command": "ls", "output": "README.md"}}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:04.000Z", "item": {"id": "item_2", "type": "agent_message", "text": "First."}}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:05.000Z", "item": {"id": "item_3", "type": "agent_message", "text": "Final."}}),
                json!({"type": "turn.completed", "timestamp": "2026-06-07T10:00:10.000Z", "usage": {"input_tokens": 100, "cached_input_tokens": 75, "output_tokens": 20, "reasoning_output_tokens": 5}}),
            ],
        );

        let full = parse_full(&codex_spec(), &path).unwrap();
        assert_eq!(
            full.tool_invocations,
            vec![ToolInvocation {
                name: "command_execution".into(),
                args: Some(json!({"command": "ls"})),
                result: Some(Value::String("README.md".into())),
                ordinal: 0,
            }]
        );
        assert_eq!(full.session_id.as_deref(), Some("thread-flat-1"));
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
        assert_eq!(full.total_tokens, Some(45)); // cached input subtracted; reasoning already in output
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
        let full = parse_full(&codex_spec(), &path).unwrap();
        assert_eq!(full.final_text, Some("Done.".into()));
        assert_eq!(full.total_tokens, None);
        assert_eq!(full.duration_ms, None);
    }

    #[test]
    fn duration_field_variant_picks_the_last_match() {
        let spec: ExtractSpec = toml::from_str(
            r#"
            [duration]
            where = { type = "result" }
            field = "elapsed_ms"
            "#,
        )
        .unwrap();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("durations.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "result", "elapsed_ms": 1_000}),
                json!({"type": "progress", "elapsed_ms": 9_999}),
                json!({"type": "result", "elapsed_ms": 2_500}),
            ],
        );
        assert_eq!(parse_full(&spec, &path).unwrap().duration_ms, Some(2_500));
    }

    #[test]
    fn matching_event_without_any_listed_token_field_leaves_tokens_null() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bare-turn.jsonl");
        write_jsonl(&path, &[json!({"type": "turn.completed"})]);
        assert_eq!(parse_full(&codex_spec(), &path).unwrap().total_tokens, None);
    }

    #[test]
    fn partial_token_fields_count_and_missing_ones_add_zero() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("partial-usage.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "turn.completed", "usage": {"input_tokens": 40, "cached_input_tokens": "unknown"}}),
                json!({"type": "turn.completed", "usage": {"output_tokens": 2}}),
                json!({"type": "turn.completed", "usage": {"input_tokens": "unknown", "cached_input_tokens": 5}}),
            ],
        );
        assert_eq!(
            parse_full(&codex_spec(), &path).unwrap().total_tokens,
            Some(42)
        );
    }

    #[test]
    fn token_subtraction_matches_codex_blended_usage_and_clamps_each_record() {
        let spec: ExtractSpec = toml::from_str(
            r#"
            [tokens]
            where = { type = "turn.completed" }
            sum = ["usage.input_tokens", "usage.output_tokens"]
            subtract = ["usage.cached_input_tokens"]
            "#,
        )
        .unwrap();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("codex-usage.jsonl");
        write_jsonl(
            &path,
            &[
                json!({
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": 886_850,
                        "cached_input_tokens": 833_024,
                        "output_tokens": 10_083,
                        "reasoning_output_tokens": 6_070
                    }
                }),
                json!({
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": 10,
                        "cached_input_tokens": 100,
                        "output_tokens": 1
                    }
                }),
            ],
        );

        assert_eq!(parse_full(&spec, &path).unwrap().total_tokens, Some(63_909));
    }

    #[test]
    fn flat_records_map_without_an_item_root() {
        let spec: ExtractSpec = toml::from_str(
            r#"
            [tools]
            where = { event = "tool" }
            name_field = "name"
            args_omit = ["event", "name", "output"]
            result_coalesce = ["output"]
            "#,
        )
        .unwrap();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("flat.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"event": "tool", "name": "grep", "pattern": "todo", "output": "3 matches"}),
                json!({"event": "text", "content": "done"}),
            ],
        );
        assert_eq!(
            parse(&spec, &path).unwrap(),
            vec![ToolInvocation {
                name: "grep".into(),
                args: Some(json!({"pattern": "todo"})),
                result: Some(Value::String("3 matches".into())),
                ordinal: 0,
            }]
        );
    }

    #[test]
    fn present_but_null_result_field_still_coalesces() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("null-result.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "item.completed", "item": {"id": "i", "type": "command_execution", "command": "true", "output": null, "error": "boom"}}),
            ],
        );
        let result = parse(&codex_spec(), &path).unwrap();
        assert_eq!(result[0].result, Some(Value::String("null".into())));
    }

    #[test]
    fn non_string_results_are_compact_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("object-result.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "item.completed", "item": {"id": "i", "type": "web_search", "result": {"hits": 2}}}),
            ],
        );
        let result = parse(&codex_spec(), &path).unwrap();
        assert_eq!(result[0].result, Some(Value::String("{\"hits\":2}".into())));
    }

    #[test]
    fn items_with_only_omitted_keys_get_null_args() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bare-item.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "item.completed", "item": {"id": "i", "type": "file_change", "status": "completed"}}),
            ],
        );
        let result = parse(&codex_spec(), &path).unwrap();
        assert_eq!(result[0].args, None);
    }

    #[test]
    fn records_missing_the_item_root_or_name_field_are_skipped() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("shapeless.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "item.completed"}),
                json!({"type": "item.completed", "item": "not an object"}),
                json!({"type": "item.completed", "item": {"id": "i", "type": 7}}),
                json!({"type": "item.completed", "item": {"id": "i"}}),
            ],
        );
        assert_eq!(parse(&codex_spec(), &path).unwrap(), vec![]);
    }

    #[test]
    fn spec_without_a_tools_mapping_yields_no_invocations() {
        let spec: ExtractSpec = toml::from_str(
            r#"
            [final_text]
            field = "text"
            "#,
        )
        .unwrap();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("no-tools.jsonl");
        write_jsonl(&path, &[json!({"text": "hello"})]);
        assert_eq!(parse(&spec, &path).unwrap(), vec![]);
        let full = parse_full(&spec, &path).unwrap();
        assert_eq!(full.tool_invocations, vec![]);
        assert_eq!(full.final_text, Some("hello".into()));
    }

    /// Acceptance benchmark for the extractor tier: the worked-example spec
    /// must parse every corpus in this module identically to the `codex-items`
    /// reference implementation.
    #[test]
    fn codex_spec_is_equivalent_to_the_codex_items_reference_parser() {
        use crate::adapters::codex::transcript::{parse_codex_events, parse_codex_events_full};

        let corpora: Vec<Vec<Value>> = vec![
            vec![
                json!({"type": "item.started", "timestamp": "2026-06-07T10:00:00.000Z", "item": {"id": "item_1", "type": "command_execution", "command": "bash -lc 'bun test'", "status": "in_progress"}}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:02.000Z", "item": {"id": "item_1", "type": "command_execution", "command": "bash -lc 'bun test'", "output": "2 pass\n0 fail", "status": "completed"}}),
                json!({"type": "item.completed", "item": {"id": "item_2", "type": "file_change", "path": "src/app.ts", "status": "completed"}}),
                json!({"type": "item.completed", "item": {"id": "item_3", "type": "agent_message", "text": "Done."}}),
            ],
            vec![
                json!({"type": "item.completed", "item": {"id": "item_1", "type": "web_search", "query": "codex events", "text": "search summary"}}),
            ],
            vec![
                json!({"type": "item.completed", "item": {"id": "a", "type": "agent_message"}}),
                json!({"type": "item.completed", "item": {"id": "b", "type": "reasoning"}}),
                json!({"type": "item.completed", "item": {"id": "c", "type": "plan_update"}}),
            ],
            vec![
                json!({"type": "thread.started", "timestamp": "2026-06-07T10:00:00.000Z"}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:03.000Z", "item": {"id": "item_1", "type": "command_execution", "command": "ls", "output": "README.md"}}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:04.000Z", "item": {"id": "item_2", "type": "agent_message", "text": "First."}}),
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:05.000Z", "item": {"id": "item_3", "type": "agent_message", "text": "Final."}}),
                json!({"type": "turn.completed", "timestamp": "2026-06-07T10:00:10.000Z", "usage": {"input_tokens": 100, "cached_input_tokens": 75, "output_tokens": 20, "reasoning_output_tokens": 5}}),
            ],
            vec![
                json!({"type": "item.completed", "timestamp": "2026-06-07T10:00:00.000Z", "item": {"id": "item_1", "type": "agent_message", "text": "Done."}}),
            ],
            vec![
                json!({"type": "item.completed", "item": {"id": "i", "type": "command_execution", "command": "true", "output": null, "error": "boom"}}),
                json!({"type": "item.completed", "item": {"id": "i", "type": "web_search", "result": {"hits": 2}}}),
                json!({"type": "item.completed", "item": {"id": "i", "type": "file_change", "status": "completed"}}),
                json!({"type": "turn.completed", "usage": {"input_tokens": 40}}),
            ],
        ];

        let spec = codex_spec();
        let dir = TempDir::new().unwrap();
        for (i, corpus) in corpora.iter().enumerate() {
            let path = dir.path().join(format!("corpus-{i}.jsonl"));
            write_jsonl(&path, corpus);
            assert_eq!(
                parse(&spec, &path).unwrap(),
                parse_codex_events(&path).unwrap(),
                "invocations diverge on corpus {i}"
            );
            assert_eq!(
                parse_full(&spec, &path).unwrap(),
                parse_codex_events_full(&path).unwrap(),
                "summaries diverge on corpus {i}"
            );
        }

        // The malformed-line corpus can't round-trip through `json!`; write it raw.
        let path = dir.path().join("corpus-malformed.jsonl");
        let good = json!({"type": "item.completed", "item": {"id": "item_1", "type": "web_search", "query": "codex exec json"}});
        fs::write(&path, format!("{good}\nnot valid json\n")).unwrap();
        assert_eq!(
            parse(&spec, &path).unwrap(),
            parse_codex_events(&path).unwrap()
        );
        assert_eq!(
            parse_full(&spec, &path).unwrap(),
            parse_codex_events_full(&path).unwrap()
        );
    }
}
