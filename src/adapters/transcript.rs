//! Harness-neutral transcript types.
//!
//! Every harness's transcript parser reduces its native events file to a
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
