//! Judge-task emission.
//!
//! Ports the `emitJudgeTasks` concern of eval-runner's `grade.ts`. For each
//! `(eval, condition)` it builds judge prompts for `llm_judge` assertions and a
//! skill-invocation meta-check (code-checked from the transcript when possible,
//! else emitted as an LLM judge task), writing `judge-tasks.json` plus the
//! per-assertion prompt files. `transcript_check` assertions are not dispatched
//! here — they are graded directly in `finalize`.

use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::json;

use crate::core::{Assertion, Harness, RunRecord, SKILL_INVOKED_META_ID, ToolInvocation};
use crate::pipeline::error::PipelineError;
use crate::pipeline::io::{now_iso8601, write_json};
use crate::validation::{SchemaName, validate_against_schema};

use super::GradeContext;

/// One judge task. `dispatch_prompt` carries the full prompt in memory but is
/// stripped from the serialized `judge-tasks.json` (the orchestrator reads it
/// from `dispatch_prompt_path` instead). `model` is always present (null or a
/// model id).
#[derive(Debug, Clone, Serialize)]
pub struct JudgeTask {
    pub eval_id: String,
    pub condition: String,
    pub assertion_id: String,
    pub rubric: String,
    pub model: Option<String>,
    pub is_meta: bool,
    pub run_record_path: String,
    pub outputs_dir: String,
    pub response_path: String,
    pub dispatch_prompt_path: String,
    #[serde(skip_serializing)]
    pub dispatch_prompt: String,
}

/// The serialized `judge-tasks.json` envelope.
#[derive(Debug, Serialize)]
struct JudgeTasksFile {
    generated: String,
    total_tasks: usize,
    meta_tasks_injected: usize,
    skipped_transcript_checks: usize,
    tasks: Vec<JudgeTask>,
}

/// What emission produced, for the CLI summary.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmitSummary {
    pub total_tasks: usize,
    pub meta_injected: usize,
    pub meta_code_checked: usize,
    pub skipped_transcript_checks: usize,
    pub skipped_missing: usize,
}

/// True when the transcript shows the `Skill` tool invoked with `input.skill`
/// equal to the staged slug. Ports `checkSkillInvokedFromTranscript`.
pub fn check_skill_invoked_from_transcript(
    invocations: &[ToolInvocation],
    staged_slug: Option<&str>,
) -> bool {
    let Some(slug) = staged_slug else {
        return false;
    };
    invocations.iter().any(|inv| {
        inv.name == "Skill"
            && inv
                .args
                .as_ref()
                .and_then(|a| a.get("skill"))
                .and_then(|v| v.as_str())
                == Some(slug)
    })
}

/// The meta-check rubric asking a judge whether the agent actually applied the
/// skill (separate from correctness). Ports `skillInvokedRubric`.
fn skill_invoked_rubric(skill_name: &str, skill_content: Option<&str>) -> String {
    let mut lines: Vec<String> = vec![
        format!(
            "The agent had access to the **{skill_name}** skill. This meta-check asks whether \
             there is evidence the agent actually applied the skill in this run — separate from \
             whether the response was correct."
        ),
        String::new(),
    ];
    if let Some(content) = skill_content {
        lines.push("# Skill content".to_string());
        lines.push(String::new());
        lines.push("```markdown".to_string());
        lines.push(content.trim().to_string());
        lines.push("```".to_string());
        lines.push(String::new());
    }
    lines.extend(
        [
            "Evidence the skill WAS applied:",
            "- The agent cites the skill by name or references specific named sections (e.g. \"Iron Law\", \"Red Flags\", \"Gate Function\", or any other distinctive heading from the skill).",
            "- The agent's response uses distinctive vocabulary or phrasing taken from the skill content.",
            "- The agent's behavior follows a specific procedural step prescribed by the skill in a way that mirrors the skill's phrasing — not just generic best practice.",
            "- The agent explicitly acknowledges following the skill's guidance.",
            "",
            "Evidence the skill was NOT applied:",
            "- The response uses only generic best-practice language unrelated to the skill's specific framing.",
            "- No vocabulary, structure, or rules from the skill content appear anywhere in the response.",
            "- The response would read identically with or without the skill loaded.",
            "",
            "Compare the agent's `final_message` against the skill content. Look for stylistic and procedural fingerprints.",
            "",
            "PASS if there is observable evidence the skill influenced the response.",
            "FAIL if there is no observable evidence — the response is indistinguishable from baseline behavior.",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    lines.join("\n")
}

/// A directory listing for the judge prompt: visible entries, dirs suffixed `/`,
/// sorted; `(empty)` when none. Ports `listOutputs`.
fn list_outputs(dir: &Path) -> String {
    let Ok(entries) = fs::read_dir(dir) else {
        return "(empty)".to_string();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" {
                return None;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Some(if is_dir { format!("{name}/") } else { name })
        })
        .collect();
    names.sort();
    if names.is_empty() {
        "(empty)".to_string()
    } else {
        names.join("\n")
    }
}

/// Assemble the full judge prompt (rubric + run record + outputs listing +
/// grading principles + where to write the verdict). Ports `buildJudgePrompt`.
fn build_judge_prompt(
    rubric: &str,
    run_record: &RunRecord,
    outputs_dir: &Path,
    response_path: &Path,
) -> String {
    let outputs_listing = if outputs_dir.exists() {
        list_outputs(outputs_dir)
    } else {
        "(none)".to_string()
    };
    let record_json = serde_json::to_string_pretty(run_record).unwrap_or_default();

    [
        "You are grading one assertion for a skill evaluation run. Be strict but fair.",
        "",
        "# Run record",
        "",
        "```json",
        &record_json,
        "```",
        "",
        "# Outputs directory contents",
        "",
        "```",
        &outputs_listing,
        "```",
        "",
        "# Assertion to grade",
        "",
        rubric,
        "",
        "# Grading principles",
        "",
        "- PASS requires concrete evidence (a direct quote or specific reference from the run record's `final_message` or outputs). Don't infer behavior not present in the record.",
        "- A correct response expressed in different words from what the assertion implies is still a PASS if the substance matches.",
        "- If the assertion is unverifiable from the available material (e.g. requires the tool-invocation list and the run record has none), return `passed: false`, `evidence: 'assertion is unverifiable from available material'`, `confidence: 1.0`.",
        "",
        "# Task",
        "",
        &format!("Write your verdict as a JSON file to: {}", response_path.display()),
        "",
        "The JSON must match this schema (exactly these keys, no extra prose in the file):",
        "",
        "```json",
        "{ \"passed\": true|false, \"evidence\": \"direct quote or reference\", \"confidence\": 0.0-1.0 }",
        "```",
        "",
        "After writing the file, your final user-facing reply should be one sentence summarising the verdict.",
    ]
    .join("\n")
}

/// Emit judge tasks + prompt files for the iteration, writing `judge-tasks.json`.
/// See the module docs for the per-assertion and meta-check behavior.
pub fn emit_judge_tasks(ctx: &GradeContext) -> Result<EmitSummary, PipelineError> {
    let conds: Vec<(String, Option<String>, Option<String>)> = ctx
        .conditions
        .conditions
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                c.skill_path.clone(),
                c.staged_skill_slug.clone().flatten(),
            )
        })
        .collect();
    let code_check_available = ctx.conditions.harness != Some(Harness::Codex);

    let mut tasks: Vec<JudgeTask> = Vec::new();
    let mut summary = EmitSummary::default();
    let mut unverifiable = 0usize;

    for ev in &ctx.evals.evals {
        let assertions = ev.assertions.as_deref().unwrap_or(&[]);
        let has_assertions = !assertions.is_empty();

        for (cond, cond_skill_path, staged_slug) in &conds {
            let cond_dir = ctx.iteration_dir.join(format!("eval-{}", ev.id)).join(cond);
            let run_record_path = cond_dir.join("run.json");
            let outputs_dir = cond_dir.join("outputs");
            let judge_responses_dir = cond_dir.join("judge-responses");
            let judge_prompts_dir = cond_dir.join("judge-prompts");

            if !run_record_path.exists() {
                eprintln!("warn: missing run.json for {}/{cond} — skipping", ev.id);
                if has_assertions {
                    summary.skipped_missing += assertions.len();
                }
                continue;
            }

            fs::create_dir_all(&judge_responses_dir)?;
            fs::create_dir_all(&judge_prompts_dir)?;
            let run_record: RunRecord = validate_against_schema(
                SchemaName::RunRecord,
                &serde_json::from_str(&fs::read_to_string(&run_record_path)?)?,
                &run_record_path.to_string_lossy(),
            )?;

            for assertion in assertions {
                let Assertion::LlmJudge(j) = assertion else {
                    // transcript_check is graded in finalize, not dispatched here.
                    unverifiable += 1;
                    continue;
                };
                let response_path = judge_responses_dir.join(format!("{}.json", j.id));
                let dispatch_prompt =
                    build_judge_prompt(&j.rubric, &run_record, &outputs_dir, &response_path);
                let prompt_path = judge_prompts_dir.join(format!("{}.txt", j.id));
                fs::write(&prompt_path, &dispatch_prompt)?;
                tasks.push(JudgeTask {
                    eval_id: ev.id.clone(),
                    condition: cond.clone(),
                    assertion_id: j.id.clone(),
                    rubric: j.rubric.clone(),
                    model: j.model.clone(),
                    is_meta: false,
                    run_record_path: run_record_path.to_string_lossy().into_owned(),
                    outputs_dir: outputs_dir.to_string_lossy().into_owned(),
                    response_path: response_path.to_string_lossy().into_owned(),
                    dispatch_prompt_path: prompt_path.to_string_lossy().into_owned(),
                    dispatch_prompt,
                });
            }

            // Skill-invocation meta-check. Negative evals (skill_should_trigger:
            // false) expect non-invocation, so they carry no meta-check.
            if cond_skill_path.is_some() && ev.skill_should_trigger != Some(false) {
                let response_path =
                    judge_responses_dir.join(format!("{SKILL_INVOKED_META_ID}.json"));
                let transcript_filled = !run_record.tool_invocations.is_empty();

                if staged_slug.is_some() && transcript_filled && code_check_available {
                    let invoked = check_skill_invoked_from_transcript(
                        &run_record.tool_invocations,
                        staged_slug.as_deref(),
                    );
                    let evidence = if invoked {
                        "Skill invocation verified from transcript.".to_string()
                    } else {
                        format!(
                            "No skill invocation found in transcript across {} transcript invocation(s).",
                            run_record.tool_invocations.len()
                        )
                    };
                    write_json(
                        &response_path,
                        &json!({
                            "passed": invoked,
                            "evidence": evidence,
                            "confidence": 1.0,
                            "grader": "transcript_check",
                        }),
                    )?;
                    summary.meta_code_checked += 1;
                } else {
                    let skill_content =
                        fs::read_to_string(cond_skill_path.as_deref().unwrap_or(""))?;
                    let rubric = skill_invoked_rubric(&ctx.evals.skill_name, Some(&skill_content));
                    let dispatch_prompt =
                        build_judge_prompt(&rubric, &run_record, &outputs_dir, &response_path);
                    let prompt_path =
                        judge_prompts_dir.join(format!("{SKILL_INVOKED_META_ID}.txt"));
                    fs::write(&prompt_path, &dispatch_prompt)?;
                    tasks.push(JudgeTask {
                        eval_id: ev.id.clone(),
                        condition: cond.clone(),
                        assertion_id: SKILL_INVOKED_META_ID.to_string(),
                        rubric,
                        model: None,
                        is_meta: true,
                        run_record_path: run_record_path.to_string_lossy().into_owned(),
                        outputs_dir: outputs_dir.to_string_lossy().into_owned(),
                        response_path: response_path.to_string_lossy().into_owned(),
                        dispatch_prompt_path: prompt_path.to_string_lossy().into_owned(),
                        dispatch_prompt,
                    });
                    summary.meta_injected += 1;
                }
            }
        }
    }

    summary.total_tasks = tasks.len();
    summary.skipped_transcript_checks = unverifiable;

    let tasks_path = ctx.iteration_dir.join("judge-tasks.json");
    write_json(
        &tasks_path,
        &JudgeTasksFile {
            generated: now_iso8601(),
            total_tasks: tasks.len(),
            meta_tasks_injected: summary.meta_injected,
            skipped_transcript_checks: unverifiable,
            tasks,
        },
    )?;

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inv(name: &str, args: Option<serde_json::Value>, ordinal: u32) -> ToolInvocation {
        ToolInvocation {
            name: name.to_string(),
            args,
            result: None,
            ordinal,
        }
    }

    #[test]
    fn true_when_skill_call_matches_slug() {
        let slug = "slow-powers-eval-1-with_skill__verification-before-completion";
        let invs = [
            inv("Bash", Some(json!({"command": "ls"})), 0),
            inv("Skill", Some(json!({"skill": slug})), 1),
            inv("Read", Some(json!({"file_path": "/tmp/x"})), 2),
        ];
        assert!(check_skill_invoked_from_transcript(&invs, Some(slug)));
    }

    #[test]
    fn false_when_no_skill_calls() {
        let invs = [
            inv("Bash", Some(json!({"command": "ls"})), 0),
            inv("Read", Some(json!({"file_path": "/tmp/x"})), 1),
        ];
        assert!(!check_skill_invoked_from_transcript(
            &invs,
            Some("slow-powers-eval-1-with_skill__foo")
        ));
    }

    #[test]
    fn false_when_skill_call_references_different_slug() {
        let slug = "slow-powers-eval-1-with_skill__verification-before-completion";
        let invs = [
            inv(
                "Skill",
                Some(json!({"skill": "slow-powers:writing-skills"})),
                0,
            ),
            inv(
                "Skill",
                Some(json!({"skill": "slow-powers-eval-2-old_skill__other"})),
                1,
            ),
        ];
        assert!(!check_skill_invoked_from_transcript(&invs, Some(slug)));
    }

    #[test]
    fn false_on_empty_invocations() {
        assert!(!check_skill_invoked_from_transcript(&[], Some("anything")));
    }

    #[test]
    fn tolerates_missing_or_malformed_skill_args() {
        let slug = "slow-powers-eval-1-with_skill__foo";
        let invs = [
            inv("Skill", None, 0),
            inv("Skill", Some(json!("not-an-object")), 1),
            inv("Skill", Some(json!({"other": "field"})), 2),
        ];
        assert!(!check_skill_invoked_from_transcript(&invs, Some(slug)));
    }
}
