//! Harness-neutral transcript types.
//!
//! Every harness's transcript parser reduces its native events file to a
//! [`TranscriptSummary`]; the pipeline consumes only this shape, never a
//! harness's raw record types.

use serde::{Deserialize, Serialize};

use crate::core::ToolInvocation;

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
