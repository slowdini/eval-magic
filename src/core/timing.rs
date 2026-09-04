//! Run timing measurements and their per-metric provenance.

use serde::{Deserialize, Serialize};

/// Token and duration measurements for a run, with provenance per metric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "TimingRecordWire")]
pub struct TimingRecord {
    /// Normalized token total. Outer `None` omits the metric; inner `None`
    /// records that the selected source could not produce a value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<Option<i64>>,
    /// Elapsed milliseconds, with the same absent-versus-unavailable shape as
    /// [`Self::total_tokens`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<Option<i64>>,
    /// Origin of [`Self::total_tokens`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_source: Option<TimingSource>,
    /// Origin of [`Self::duration_ms`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_source: Option<TimingSource>,
}

impl TimingRecord {
    /// Returns the token provenance, defaulting historical records with no
    /// source field to live completion-event capture.
    pub fn effective_token_source(&self) -> TimingSource {
        self.token_source.unwrap_or(TimingSource::CompletionEvent)
    }

    /// Returns the duration provenance, with the same historical default as
    /// [`Self::effective_token_source`].
    pub fn effective_duration_source(&self) -> TimingSource {
        self.duration_source
            .unwrap_or(TimingSource::CompletionEvent)
    }
}

/// Deserialization-only shape that accepts the historical shared `source`.
#[derive(Deserialize)]
struct TimingRecordWire {
    #[serde(default)]
    total_tokens: Option<Option<i64>>,
    #[serde(default)]
    duration_ms: Option<Option<i64>>,
    #[serde(default)]
    token_source: Option<TimingSource>,
    #[serde(default)]
    duration_source: Option<TimingSource>,
    #[serde(default)]
    source: Option<TimingSource>,
}

impl From<TimingRecordWire> for TimingRecord {
    fn from(wire: TimingRecordWire) -> Self {
        let legacy_source = wire.source.unwrap_or(TimingSource::CompletionEvent);
        let token_source = wire
            .token_source
            .or_else(|| wire.total_tokens.is_some().then_some(legacy_source));
        let duration_source = wire
            .duration_source
            .or_else(|| wire.duration_ms.is_some().then_some(legacy_source));
        Self {
            total_tokens: wire.total_tokens,
            duration_ms: wire.duration_ms,
            token_source,
            duration_source,
        }
    }
}

/// Provenance of one [`TimingRecord`] metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimingSource {
    /// A live harness completion event created the timing file.
    CompletionEvent,
    /// The eval-magic runner measured the harness subprocess.
    Runner,
    /// Ingest derived the metric from persisted harness events.
    Transcript,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn keeps_metric_provenance_independent() {
        let timing: TimingRecord = serde_json::from_value(json!({
            "total_tokens": 12,
            "duration_ms": 34,
            "token_source": "transcript",
            "duration_source": "runner"
        }))
        .unwrap();

        assert_eq!(timing.effective_token_source(), TimingSource::Transcript);
        assert_eq!(timing.effective_duration_source(), TimingSource::Runner);
        assert_eq!(
            serde_json::to_value(timing).unwrap(),
            json!({
                "total_tokens": 12,
                "duration_ms": 34,
                "token_source": "transcript",
                "duration_source": "runner"
            })
        );
    }

    #[test]
    fn reads_legacy_shared_source_and_rewrites_it_per_metric() {
        let timing: TimingRecord = serde_json::from_value(json!({
            "total_tokens": 12,
            "duration_ms": 34,
            "source": "transcript"
        }))
        .unwrap();

        assert_eq!(timing.effective_token_source(), TimingSource::Transcript);
        assert_eq!(timing.effective_duration_source(), TimingSource::Transcript);
        assert_eq!(
            serde_json::to_value(timing).unwrap(),
            json!({
                "total_tokens": 12,
                "duration_ms": 34,
                "token_source": "transcript",
                "duration_source": "transcript"
            }),
            "rewriting a legacy record emits unambiguous per-metric provenance"
        );
    }

    #[test]
    fn reads_legacy_missing_source_as_completion_event() {
        let timing: TimingRecord = serde_json::from_value(json!({
            "total_tokens": 12,
            "duration_ms": 34
        }))
        .unwrap();

        assert_eq!(
            serde_json::to_value(timing).unwrap(),
            json!({
                "total_tokens": 12,
                "duration_ms": 34,
                "token_source": "completion-event",
                "duration_source": "completion-event"
            })
        );
    }

    #[test]
    fn timing_source_kebab_roundtrips() {
        let value = serde_json::to_value(TimingSource::CompletionEvent).unwrap();
        assert_eq!(value, Value::String("completion-event".into()));
        let back: TimingSource = serde_json::from_value(value).unwrap();
        assert_eq!(back, TimingSource::CompletionEvent);
    }
}
