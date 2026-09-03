//! Grading finalize.
//!
//! For each
//! `(eval, condition)` it grades `transcript_check` assertions directly, folds in
//! persisted `command_check` results, deterministic `diff_scope` thresholds,
//! and the `llm_judge` responses written by the orchestrator (a missing response
//! fails only that verdict), assembles the skill-invocation meta result, and
//! writes a schema-valid `grading.json` with binary or sampled vote summaries.

use std::fs;

use serde::Deserialize;

use crate::adapters::{adapter_for, all_tool_vocabulary};
use crate::core::fs::write_json;
use crate::core::{
    Assertion, AssertionResult, BinaryGradingSummary, GradedAssertionResult, Grader, GradingResult,
    GradingSummary, JudgeSampleResult, JudgeVotes, MetaResult, MetaSummary, RunRecord,
    SKILL_INVOKED_META_ID, SampledAssertionResult, SampledGradingSummary, ToolInvocation,
};
use crate::pipeline::DiffScopeMetrics;
use crate::pipeline::error::PipelineError;
use crate::pipeline::slots::run_slots;
use crate::validation::{SchemaName, validate_against_schema};

use super::GradeContext;
use super::command_check::CommandCheckResult;
use super::diff_scope::grade_diff_scope;
use super::judge_tasks::meta_response_stem;
use super::transcript_check::{ToolNaming, grade_transcript_check_with_context};

/// What finalize graded, for the CLI summary.
#[derive(Debug, Default, Clone)]
pub struct FinalizeSummary {
    pub total_graded: usize,
    pub total_meta_graded: usize,
    pub total_unverifiable: usize,
    pub meta_failures: usize,
    /// Per-item detail behind the counters above (which judge response was
    /// missing). Collected here rather than printed so the stage stays silent
    /// and the CLI owns every user-facing line.
    pub warnings: Vec<String>,
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

/// Fold runner checks and judge responses into a `grading.json` per
/// `(eval, condition)`. See the module docs for the per-assertion behavior.
pub fn finalize(ctx: &GradeContext) -> Result<FinalizeSummary, PipelineError> {
    let default_skill_name = ctx.evals.skill_names().first().cloned().unwrap_or_default();
    let conds: Vec<(String, Vec<String>, bool)> = ctx
        .conditions
        .conditions
        .iter()
        .map(|c| {
            let is_multi = c.skills.is_some();
            let skills = c.skills.as_ref().map_or_else(
                || {
                    c.skill_path
                        .as_ref()
                        .map(|_| vec![default_skill_name.clone()])
                        .unwrap_or_default()
                },
                |skills| skills.iter().map(|skill| skill.name.clone()).collect(),
            );
            (c.name.clone(), skills, is_multi)
        })
        .collect();

    let mut summary = FinalizeSummary::default();
    // The run's own descriptor decides which role each native tool name plays;
    // the registry-wide union supplies the portable spellings of that role, so
    // one authored pattern grades the same on every harness (#308).
    let transcript_vocabulary = ctx
        .conditions
        .harness
        .map(|harness| adapter_for(harness).tool_vocabulary())
        .unwrap_or_default();
    let naming = ToolNaming::new(&transcript_vocabulary, all_tool_vocabulary());

    for ev in &ctx.evals.evals {
        let assertions = ev.assertions.as_deref().unwrap_or(&[]);
        let has_assertions = !assertions.is_empty();

        for (cond, condition_skills, multi_skill) in &conds {
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

                let mut assertion_results: Vec<GradedAssertionResult> = Vec::new();
                if has_assertions {
                    for assertion in assertions {
                        match assertion {
                            Assertion::TranscriptCheck(tc) => {
                                let invocations: &[ToolInvocation] = run_record
                                    .as_ref()
                                    .map(|r| r.tool_invocations.as_slice())
                                    .unwrap_or(&[]);
                                let conversation = run_record
                                    .as_ref()
                                    .and_then(|run| run.conversation.as_ref());
                                assertion_results.push(
                                    grade_transcript_check_with_context(
                                        tc,
                                        invocations,
                                        conversation,
                                        &naming,
                                    )
                                    .into(),
                                );
                                let unverifiable = match tc.check.as_str() {
                                    "assistant_message_matches" => conversation.is_none(),
                                    _ => invocations.is_empty(),
                                };
                                if unverifiable {
                                    summary.total_unverifiable += 1;
                                } else {
                                    summary.total_graded += 1;
                                }
                            }
                            Assertion::LlmJudge(j) => {
                                let sample_count =
                                    j.samples.or(ctx.conditions.judge_samples).unwrap_or(1);
                                if sample_count > 1 {
                                    let mut judge_samples =
                                        Vec::with_capacity(sample_count as usize);
                                    for sample_index in 1..=sample_count {
                                        let response_path = judge_responses_dir
                                            .join(format!("{}__sample-{sample_index}.json", j.id));
                                        if !response_path.exists() {
                                            summary.warnings.push(format!(
                                                "missing judge response: {} (sample will be FAIL)",
                                                response_path.display()
                                            ));
                                            judge_samples.push(JudgeSampleResult {
                                                sample_index,
                                                passed: false,
                                                evidence: format!(
                                                    "judge response missing at {}",
                                                    response_path.display()
                                                ),
                                                confidence: 0.0,
                                            });
                                            continue;
                                        }
                                        let response: JudgeResponse = serde_json::from_str(
                                            &fs::read_to_string(&response_path)?,
                                        )?;
                                        judge_samples.push(JudgeSampleResult {
                                            sample_index,
                                            passed: response.passed,
                                            evidence: response.evidence.unwrap_or_default(),
                                            confidence: response.confidence.unwrap_or(0.0),
                                        });
                                    }
                                    let passed =
                                        judge_samples.iter().filter(|sample| sample.passed).count()
                                            as u32;
                                    let proportion = f64::from(passed) / f64::from(sample_count);
                                    assertion_results.push(GradedAssertionResult::Sampled(
                                        SampledAssertionResult {
                                            id: j.id.clone(),
                                            grader: Grader::LlmJudge,
                                            votes: JudgeVotes {
                                                passed,
                                                failed: sample_count - passed,
                                                total: sample_count,
                                                proportion,
                                                pass_power_k: proportion
                                                    .powf(f64::from(sample_count)),
                                            },
                                            judge_samples,
                                        },
                                    ));
                                    summary.total_graded += 1;
                                    continue;
                                }
                                let response_path =
                                    judge_responses_dir.join(format!("{}.json", j.id));
                                if !response_path.exists() {
                                    summary.warnings.push(format!(
                                        "missing judge response: {} (assertion will be FAIL)",
                                        response_path.display()
                                    ));
                                    assertion_results.push(
                                        AssertionResult {
                                            id: j.id.clone(),
                                            passed: false,
                                            evidence: format!(
                                                "judge response missing at {}",
                                                response_path.display()
                                            ),
                                            confidence: Some(0.0),
                                            grader: Some(Grader::LlmJudge),
                                        }
                                        .into(),
                                    );
                                    continue;
                                }
                                let response: JudgeResponse =
                                    serde_json::from_str(&fs::read_to_string(&response_path)?)?;
                                assertion_results.push(
                                    AssertionResult {
                                        id: j.id.clone(),
                                        passed: response.passed,
                                        evidence: response.evidence.unwrap_or_default(),
                                        confidence: Some(response.confidence.unwrap_or(0.0)),
                                        grader: Some(Grader::LlmJudge),
                                    }
                                    .into(),
                                );
                                summary.total_graded += 1;
                            }
                            Assertion::CommandCheck(check) => {
                                let result_path = slot
                                    .dir
                                    .join("command-checks")
                                    .join(format!("{}.json", check.id));
                                if !result_path.exists() {
                                    return Err(PipelineError::Message(format!(
                                        "missing command_check result: {}. Run ingest (or grade without --finalize) before finalize.",
                                        result_path.display()
                                    )));
                                }
                                let result: CommandCheckResult = validate_against_schema(
                                    SchemaName::CommandCheck,
                                    &serde_json::from_str(&fs::read_to_string(&result_path)?)?,
                                    &result_path.to_string_lossy(),
                                )?;
                                assertion_results.push(
                                    AssertionResult {
                                        id: check.id.clone(),
                                        passed: result.passed,
                                        evidence: result.evidence,
                                        confidence: Some(1.0),
                                        grader: Some(Grader::CommandCheck),
                                    }
                                    .into(),
                                );
                                summary.total_graded += 1;
                            }
                            Assertion::DiffScope(check) => {
                                let result_path = slot.dir.join("diff-scope.json");
                                if !result_path.exists() {
                                    return Err(PipelineError::Message(format!(
                                        "missing diff_scope result: {}. Run ingest before finalize; if this iteration predates diff-scope baselines, rebuild it first.",
                                        result_path.display()
                                    )));
                                }
                                let metrics: DiffScopeMetrics = validate_against_schema(
                                    SchemaName::DiffScope,
                                    &serde_json::from_str(&fs::read_to_string(&result_path)?)?,
                                    &result_path.to_string_lossy(),
                                )?;
                                assertion_results.push(grade_diff_scope(check, metrics).into());
                                summary.total_graded += 1;
                            }
                        }
                    }
                }

                // Mirror the emit gate: negative evals carry no meta-check.
                let mut meta_results: Vec<MetaResult> = Vec::new();
                let mut scalar_meta_failed = false;
                if !condition_skills.is_empty() && ev.skill_should_trigger != Some(false) {
                    for (index, skill_name) in condition_skills.iter().enumerate() {
                        let stem = meta_response_stem(index, *multi_skill);
                        let response_path = judge_responses_dir.join(format!("{stem}.json"));
                        if response_path.exists() {
                            let response: JudgeResponse =
                                serde_json::from_str(&fs::read_to_string(&response_path)?)?;
                            let passed = response.passed;
                            meta_results.push(MetaResult {
                                id: SKILL_INVOKED_META_ID.to_string(),
                                skill_name: (*multi_skill).then(|| skill_name.clone()),
                                passed,
                                evidence: response.evidence.unwrap_or_default(),
                                confidence: Some(response.confidence.unwrap_or(0.0)),
                                grader: Some(response.grader.unwrap_or(Grader::LlmJudge)),
                            });
                            summary.total_meta_graded += 1;
                            scalar_meta_failed |= !*multi_skill && !passed;
                        } else {
                            summary.warnings.push(if *multi_skill {
                                format!(
                                    "missing skill-invocation meta response for '{}': {}",
                                    skill_name,
                                    response_path.display()
                                )
                            } else {
                                format!(
                                    "missing skill-invocation meta response: {}",
                                    response_path.display()
                                )
                            });
                            meta_results.push(MetaResult {
                                id: SKILL_INVOKED_META_ID.to_string(),
                                skill_name: (*multi_skill).then(|| skill_name.clone()),
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
                }

                let total = assertion_results.len() as u32;
                let meta_len = meta_results.len() as u32;
                let meta_passed = meta_results.iter().filter(|r| r.passed).count() as u32;
                let has_meta = !meta_results.is_empty();
                let skill_invoked = has_meta.then(|| meta_results.iter().any(|r| r.passed));
                if (*multi_skill && skill_invoked == Some(false)) || scalar_meta_failed {
                    summary.meta_failures += 1;
                }

                let has_sampled = assertion_results
                    .iter()
                    .any(|result| matches!(result, GradedAssertionResult::Sampled(_)));
                let grading_summary = if has_sampled {
                    let divisor = f64::from(total);
                    let vote_proportion = if total == 0 {
                        0.0
                    } else {
                        assertion_results
                            .iter()
                            .map(GradedAssertionResult::vote_proportion)
                            .sum::<f64>()
                            / divisor
                    };
                    let pass_power_k = if total == 0 {
                        0.0
                    } else {
                        assertion_results
                            .iter()
                            .map(GradedAssertionResult::pass_power_k)
                            .sum::<f64>()
                            / divisor
                    };
                    GradingSummary::Sampled(SampledGradingSummary {
                        total,
                        pass_rate: vote_proportion,
                        vote_proportion,
                        pass_power_k,
                    })
                } else {
                    let passed = assertion_results
                        .iter()
                        .filter(|result| result.vote_proportion() == 1.0)
                        .count() as u32;
                    GradingSummary::Binary(BinaryGradingSummary {
                        passed,
                        failed: total - passed,
                        total,
                        pass_rate: if total == 0 {
                            0.0
                        } else {
                            f64::from(passed) / f64::from(total)
                        },
                    })
                };

                let grading = GradingResult {
                    assertion_results,
                    meta_results: has_meta.then_some(meta_results),
                    summary: grading_summary,
                    meta_summary: has_meta.then_some(MetaSummary {
                        passed: meta_passed,
                        failed: meta_len - meta_passed,
                        total: meta_len,
                        skill_invoked,
                    }),
                    assertion_source: Some(ctx.assertion_source.clone()),
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
