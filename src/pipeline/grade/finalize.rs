//! Grading finalize.
//!
//! For each
//! `(eval, condition)` it grades `transcript_check` assertions directly, folds in
//! the `llm_judge` responses written by the orchestrator (missing → FAIL),
//! assembles the skill-invocation meta result, and writes a schema-valid
//! `grading.json` with pass/fail summaries.

use std::fs;

use serde::Deserialize;

use crate::core::{
    Assertion, AssertionResult, Grader, GradingResult, GradingSummary, MetaSummary, RunRecord,
    SKILL_INVOKED_META_ID, ToolInvocation,
};
use crate::pipeline::error::PipelineError;
use crate::pipeline::io::write_json;
use crate::pipeline::slots::run_slots;
use crate::validation::{SchemaName, validate_against_schema};

use super::GradeContext;
use super::transcript_check::grade_transcript_check;

/// What finalize graded, for the CLI summary.
#[derive(Debug, Default, Clone, Copy)]
pub struct FinalizeSummary {
    pub total_graded: usize,
    pub total_meta_graded: usize,
    pub total_unverifiable: usize,
    pub meta_failures: usize,
}

/// A judge's verdict file. All fields tolerate absence (a sloppy judge response
/// degrades to FAIL/0 rather than erroring the stage).
#[derive(Debug, Deserialize)]
struct JudgeResponse {
    #[serde(default)]
    passed: bool,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    grader: Option<Grader>,
}

/// Fold judge responses + transcript checks into a `grading.json` per
/// `(eval, condition)`. See the module docs for the per-assertion behavior.
pub fn finalize(ctx: &GradeContext) -> Result<FinalizeSummary, PipelineError> {
    let conds: Vec<(String, Option<String>)> = ctx
        .conditions
        .conditions
        .iter()
        .map(|c| (c.name.clone(), c.skill_path.clone()))
        .collect();

    let mut summary = FinalizeSummary::default();

    for ev in &ctx.evals.evals {
        let assertions = ev.assertions.as_deref().unwrap_or(&[]);
        let has_assertions = !assertions.is_empty();

        for (cond, cond_skill_path) in &conds {
            let cond_dir = ctx.iteration_dir.join(format!("eval-{}", ev.id)).join(cond);
            if !cond_dir.exists() {
                continue;
            }
            for slot in run_slots(&cond_dir) {
                let judge_responses_dir = slot.dir.join("judge-responses");
                let grading_path = slot.dir.join("grading.json");

                let run_record_path = slot.dir.join("run.json");
                let run_record: Option<RunRecord> = if run_record_path.exists() {
                    Some(validate_against_schema(
                        SchemaName::RunRecord,
                        &serde_json::from_str(&fs::read_to_string(&run_record_path)?)?,
                        &run_record_path.to_string_lossy(),
                    )?)
                } else {
                    None
                };

                let mut assertion_results: Vec<AssertionResult> = Vec::new();
                if has_assertions {
                    for assertion in assertions {
                        match assertion {
                            Assertion::TranscriptCheck(tc) => {
                                let invocations: &[ToolInvocation] = run_record
                                    .as_ref()
                                    .map(|r| r.tool_invocations.as_slice())
                                    .unwrap_or(&[]);
                                assertion_results.push(grade_transcript_check(tc, invocations));
                                if invocations.is_empty() {
                                    summary.total_unverifiable += 1;
                                } else {
                                    summary.total_graded += 1;
                                }
                            }
                            Assertion::LlmJudge(j) => {
                                let response_path =
                                    judge_responses_dir.join(format!("{}.json", j.id));
                                if !response_path.exists() {
                                    eprintln!(
                                        "warn: missing judge response: {} (assertion will be FAIL)",
                                        response_path.display()
                                    );
                                    assertion_results.push(AssertionResult {
                                        id: j.id.clone(),
                                        passed: false,
                                        evidence: format!(
                                            "judge response missing at {}",
                                            response_path.display()
                                        ),
                                        confidence: Some(0.0),
                                        grader: Some(Grader::LlmJudge),
                                    });
                                    continue;
                                }
                                let response: JudgeResponse =
                                    serde_json::from_str(&fs::read_to_string(&response_path)?)?;
                                assertion_results.push(AssertionResult {
                                    id: j.id.clone(),
                                    passed: response.passed,
                                    evidence: response.evidence.unwrap_or_default(),
                                    confidence: Some(response.confidence.unwrap_or(0.0)),
                                    grader: Some(Grader::LlmJudge),
                                });
                                summary.total_graded += 1;
                            }
                        }
                    }
                }

                // Mirror the emit gate: negative evals carry no meta-check.
                let mut meta_results: Vec<AssertionResult> = Vec::new();
                if cond_skill_path.is_some() && ev.skill_should_trigger != Some(false) {
                    let response_path =
                        judge_responses_dir.join(format!("{SKILL_INVOKED_META_ID}.json"));
                    if response_path.exists() {
                        let response: JudgeResponse =
                            serde_json::from_str(&fs::read_to_string(&response_path)?)?;
                        let passed = response.passed;
                        meta_results.push(AssertionResult {
                            id: SKILL_INVOKED_META_ID.to_string(),
                            passed,
                            evidence: response.evidence.unwrap_or_default(),
                            confidence: Some(response.confidence.unwrap_or(0.0)),
                            grader: Some(response.grader.unwrap_or(Grader::LlmJudge)),
                        });
                        summary.total_meta_graded += 1;
                        if !passed {
                            summary.meta_failures += 1;
                        }
                    } else {
                        eprintln!(
                            "warn: missing skill-invocation meta response: {}",
                            response_path.display()
                        );
                        meta_results.push(AssertionResult {
                            id: SKILL_INVOKED_META_ID.to_string(),
                            passed: false,
                            evidence: format!(
                                "meta judge response missing at {}",
                                response_path.display()
                            ),
                            confidence: Some(0.0),
                            grader: Some(Grader::LlmJudge),
                        });
                    }
                }

                let passed = assertion_results.iter().filter(|r| r.passed).count() as u32;
                let total = assertion_results.len() as u32;
                let meta_len = meta_results.len() as u32;
                let meta_passed = meta_results.iter().filter(|r| r.passed).count() as u32;
                let has_meta = !meta_results.is_empty();
                let skill_invoked = has_meta.then(|| meta_results.iter().all(|r| r.passed));

                let grading = GradingResult {
                    assertion_results,
                    meta_results: has_meta.then_some(meta_results),
                    summary: GradingSummary {
                        passed,
                        failed: total - passed,
                        total,
                        pass_rate: if total == 0 {
                            0.0
                        } else {
                            f64::from(passed) / f64::from(total)
                        },
                    },
                    meta_summary: has_meta.then_some(MetaSummary {
                        passed: meta_passed,
                        failed: meta_len - meta_passed,
                        total: meta_len,
                        skill_invoked,
                    }),
                };

                validate_against_schema::<serde_json::Value>(
                    SchemaName::Grading,
                    &serde_json::to_value(&grading)?,
                    &grading_path.to_string_lossy(),
                )?;
                write_json(&grading_path, &grading)?;
            }
        }
    }

    Ok(summary)
}
