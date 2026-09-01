//! Grading artifact types shared by finalization and aggregation.

use serde::{Deserialize, Serialize};

/// The result of grading one binary assertion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssertionResult {
    pub id: String,
    pub passed: bool,
    pub evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grader: Option<Grader>,
}

/// A framework-injected binary result. Multi-skill invocation checks name the
/// treatment member; scalar artifacts omit the field and retain their legacy
/// shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetaResult {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_name: Option<String>,
    pub passed: bool,
    pub evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grader: Option<Grader>,
}

/// One verdict inside a multi-sample LLM assertion result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgeSampleResult {
    pub sample_index: u32,
    pub passed: bool,
    pub evidence: String,
    pub confidence: f64,
}

/// Vote totals and derived consistency metrics for one sampled assertion.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct JudgeVotes {
    pub passed: u32,
    pub failed: u32,
    pub total: u32,
    pub proportion: f64,
    pub pass_power_k: f64,
}

/// A substantive LLM assertion represented by its independent judge verdicts,
/// without an ambiguous assertion-level boolean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampledAssertionResult {
    pub id: String,
    pub grader: Grader,
    pub votes: JudgeVotes,
    pub judge_samples: Vec<JudgeSampleResult>,
}

/// A substantive assertion result. The untagged variants preserve the exact
/// legacy boolean shape while admitting sampled LLM results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GradedAssertionResult {
    Sampled(SampledAssertionResult),
    Binary(AssertionResult),
}

impl From<AssertionResult> for GradedAssertionResult {
    fn from(result: AssertionResult) -> Self {
        Self::Binary(result)
    }
}

impl GradedAssertionResult {
    pub fn id(&self) -> &str {
        match self {
            Self::Sampled(result) => &result.id,
            Self::Binary(result) => &result.id,
        }
    }

    pub fn vote_proportion(&self) -> f64 {
        match self {
            Self::Sampled(result) => result.votes.proportion,
            Self::Binary(result) => {
                if result.passed {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    pub fn pass_power_k(&self) -> f64 {
        match self {
            Self::Sampled(result) => result.votes.pass_power_k,
            Self::Binary(result) => {
                if result.passed {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

/// Which grader produced an assertion result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grader {
    TranscriptCheck,
    LlmJudge,
    CommandCheck,
    DiffScope,
}

/// Which `evals.json` supplied the assertions a grading measured against.
///
/// The treatment stays frozen at what ran, but assertions are the measuring
/// instrument and are authored after the dispatch they grade, so the file they
/// came from is recorded rather than assumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionSource {
    /// The `evals.json` the graded assertions came from.
    pub path: String,
    /// Digest over every graded eval's grading fields, so two gradings can be
    /// compared without diffing the file they were read from.
    pub digest: String,
    /// True when the live file replaced the assertions the run froze.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refreshed: bool,
}

/// The full grading output for one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradingResult {
    pub assertion_results: Vec<GradedAssertionResult>,
    // Substantive results + summary first, then the optional meta block —
    // grading.json reads as "the verdict, then the validity check on it".
    pub summary: GradingSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta_results: Option<Vec<MetaResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta_summary: Option<MetaSummary>,
    /// Which `evals.json` supplied the assertions above. Absent in gradings
    /// written before the source was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assertion_source: Option<AssertionSource>,
}

/// Legacy pass/fail tallies for an entirely binary grading.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BinaryGradingSummary {
    pub passed: u32,
    pub failed: u32,
    pub total: u32,
    pub pass_rate: f64,
}

/// Equal-assertion-weight endpoints for a grading containing sampled judges.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SampledGradingSummary {
    pub total: u32,
    /// Compatibility alias for `vote_proportion`.
    pub pass_rate: f64,
    pub vote_proportion: f64,
    pub pass_power_k: f64,
}

/// Per-run grading summary, preserving the exact legacy shape until an
/// assertion requests more than one judge verdict.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GradingSummary {
    Sampled(SampledGradingSummary),
    Binary(BinaryGradingSummary),
}

impl GradingSummary {
    pub fn pass_rate(self) -> f64 {
        match self {
            Self::Sampled(summary) => summary.pass_rate,
            Self::Binary(summary) => summary.pass_rate,
        }
    }

    pub fn vote_proportion(self) -> Option<f64> {
        match self {
            Self::Sampled(summary) => Some(summary.vote_proportion),
            Self::Binary(_) => None,
        }
    }

    pub fn pass_power_k(self) -> Option<f64> {
        match self {
            Self::Sampled(summary) => Some(summary.pass_power_k),
            Self::Binary(_) => None,
        }
    }
}

/// Tallies for the meta-assertions, plus the skill-invocation determination.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MetaSummary {
    pub passed: u32,
    pub failed: u32,
    pub total: u32,
    /// `None` (serialized `null`) when invocation could not be determined.
    pub skill_invoked: Option<bool>,
}
