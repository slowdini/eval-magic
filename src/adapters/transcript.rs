//! Harness-neutral transcript types.
//!
//! Every harness's primary transcript reader reduces its native events file to a
//! [`TranscriptSummary`]; the pipeline consumes only this shape, never a
//! harness's raw record types.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

use crate::core::ToolInvocation;

/// One ordered, user-visible or tool event parsed from a harness transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptEvent {
    AssistantMessage {
        ordinal: u32,
        text: String,
    },
    ToolInvocation {
        ordinal: u32,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
    },
}

/// One tool call the harness refused to execute.
///
/// Deliberately compact and privacy-safe, like
/// [`GuardDenialRecord`](crate::sandbox::GuardDenialRecord): input *keys*, never
/// input values, so a refused `Write` cannot spill a file body into a report.
/// The refusal `reason` is harness-authored text and often names the refused
/// command on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDenial {
    pub tool: String,
    /// The harness's refusal text, when it surfaces one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Sorted keys of the refused tool input.
    pub input_keys: Vec<String>,
}

/// Read a JSONL file, deserializing each non-blank line as `T` and silently
/// skipping malformed lines (a partial transcript still yields its parseable
/// records).
pub(crate) fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Vec<T>> {
    let raw = fs::read_to_string(path)?;
    Ok(raw
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<T>(line).ok())
        .collect())
}

/// One plugin a dispatch loaded, as its own transcript reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedPlugin {
    /// The namespace the harness prefixes this plugin's skills with
    /// (`slow-powers` in `slow-powers:hardening-plans`).
    pub name: String,
    /// The identity an operator disables — the `enabledPlugins` key, e.g.
    /// `slow-powers@slowdini`. Absent when the harness reports only a name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// What one dispatch reported it could discover, read from that dispatch's own
/// transcript.
///
/// Parsers return `Option<Self>`, and the distinction is load-bearing: `None`
/// means the transcript reports **no evidence** either way, while a `Some` whose
/// vectors are empty is a harness positively reporting an empty surface — proof
/// that nothing live loaded. Collapsing the two would turn "we cannot tell" into
/// "nothing was there", which is exactly the wrong direction to guess in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSurface {
    /// Runtime skill identifiers the session could see: staged slugs, natural
    /// names, and namespaced plugin skills (`<plugin>:<skill>`).
    pub advertised_skills: Vec<String>,
    pub loaded_plugins: Vec<LoadedPlugin>,
}

/// A transcript boiled down to the artifacts the pipeline needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSummary {
    pub tool_invocations: Vec<ToolInvocation>,
    /// Ordered assistant-message and tool events, for multi-turn gating and
    /// cross-event grading.
    #[serde(default)]
    pub events: Vec<TranscriptEvent>,
    /// Native conversation/session identifier used to resume the next turn.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Harness-normalized total token usage, as reported by the persisted
    /// transcript.
    pub total_tokens: Option<i64>,
    /// Wall-clock duration when the transcript exposes a reliable duration or
    /// enough timestamps to derive one.
    pub duration_ms: Option<i64>,
    /// Concatenated text blocks of the last assistant message.
    pub final_text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::read_jsonl;
    use serde_json::Value;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn read_jsonl_skips_malformed_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        fs::write(&path, "{\"a\":1}\nnot json\n{\"a\":2}\n").unwrap();
        let values: Vec<Value> = read_jsonl(&path).unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["a"], 1);
        assert_eq!(values[1]["a"], 2);
    }

    #[test]
    fn read_jsonl_skips_blank_and_whitespace_only_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        fs::write(&path, "{\"a\":1}\n\n   \n\t\n{\"a\":2}\n").unwrap();
        let values: Vec<Value> = read_jsonl(&path).unwrap();
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn read_jsonl_errors_on_a_missing_file() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("absent.jsonl");
        assert!(read_jsonl::<Value>(&missing).is_err());
    }
}
