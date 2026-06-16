//! The `grade` subcommand — judge-task emission and `--finalize` folding.

use crate::helpers::{canonical_root, skill_eval};
use assert_cmd::Command;
use predicates::str::contains;
use std::fs;

/// Write `<skill_sub>/SKILL.md` and `<skill_sub>/evals/evals.json`.
fn write_skill(skill_sub: &std::path::Path, skill_md: &str, evals: &serde_json::Value) {
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    fs::write(skill_sub.join("SKILL.md"), skill_md).unwrap();
    fs::write(
        skill_sub.join("evals").join("evals.json"),
        serde_json::to_string_pretty(evals).unwrap(),
    )
    .unwrap();
}

/// Run `grade` (optionally `--finalize`) for skill `mr-review`, iteration 1.
fn grade_cmd(cwd: &std::path::Path, skill_dir: &std::path::Path, harness: Option<&str>) -> Command {
    let mut cmd = skill_eval();
    cmd.current_dir(cwd)
        .arg("grade")
        .arg("--skill-dir")
        .arg(skill_dir)
        .arg("--skill")
        .arg("mr-review")
        .arg("--iteration")
        .arg("1");
    if let Some(h) = harness {
        cmd.arg("--harness").arg(h);
    }
    cmd
}

/// `grade` (emit): a Codex staged run routes the skill-invocation meta-check to an
/// LLM judge task whose prompt embeds the SKILL.md content (no code-check).
#[test]
fn grade_codex_staged_run_uses_llm_meta_check_with_skill_content() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nUse the MERGE-RISK-LADDER before writing the final review.",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Review this MR.", "expected_output": "Agent reviews systematically."}
        ]}),
    );
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    let cond_dir = iteration_dir.join("eval-pos-eval").join("with_skill");
    fs::create_dir_all(&cond_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [{"name": "with_skill", "skill_path": skill_md, "staged_skill_slug": "slow-powers-eval-1-with_skill__mr-review"}],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "codex",
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        cond_dir.join("run.json"),
        serde_json::to_string(&json!({
            "eval_id": "pos-eval", "condition": "with_skill", "skill_path": skill_md,
            "prompt": "p", "files": [], "final_message": "I reviewed the MR.",
            "tool_invocations": [{"name": "command_execution", "args": {"command": "ls"}, "ordinal": 0}],
            "total_tokens": 100, "duration_ms": 1000,
        }))
        .unwrap(),
    )
    .unwrap();

    grade_cmd(&cwd, &skill_dir, Some("codex"))
        .assert()
        .success();

    assert!(
        !cond_dir
            .join("judge-responses")
            .join("__skill_invoked.json")
            .exists()
    );
    let tasks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("judge-tasks.json")).unwrap())
            .unwrap();
    let has_meta = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["assertion_id"] == json!("__skill_invoked") && t["is_meta"] == json!(true));
    assert!(has_meta, "expected a meta judge task");
    let prompt =
        fs::read_to_string(cond_dir.join("judge-prompts").join("__skill_invoked.txt")).unwrap();
    assert!(prompt.contains("MERGE-RISK-LADDER"));
}

/// `grade` (emit): evals marked `skill_should_trigger: false` get no meta-check.
#[test]
fn grade_omits_meta_check_for_negative_evals() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Fix the failing build.", "expected_output": "debugs",
             "assertions": [{"id": "a1", "type": "llm_judge", "rubric": "Did it debug?"}]},
            {"id": "neg-eval", "prompt": "Add a --verbose flag.", "expected_output": "feature",
             "skill_should_trigger": false,
             "assertions": [{"id": "a2", "type": "llm_judge", "rubric": "Did it avoid debugging?"}]}
        ]}),
    );
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    fs::create_dir_all(&iteration_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [
                {"name": "with_skill", "skill_path": skill_md},
                {"name": "without_skill", "skill_path": null},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    for eval_id in ["pos-eval", "neg-eval"] {
        for cond in ["with_skill", "without_skill"] {
            let cond_dir = iteration_dir.join(format!("eval-{eval_id}")).join(cond);
            fs::create_dir_all(&cond_dir).unwrap();
            let skill_path = if cond == "with_skill" {
                json!(skill_md)
            } else {
                json!(null)
            };
            fs::write(
                cond_dir.join("run.json"),
                serde_json::to_string(&json!({
                    "eval_id": eval_id, "condition": cond, "skill_path": skill_path,
                    "prompt": "p", "files": [], "final_message": "done",
                    "tool_invocations": [], "total_tokens": 100, "duration_ms": 1000,
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }

    grade_cmd(&cwd, &skill_dir, None).assert().success();

    let tasks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("judge-tasks.json")).unwrap())
            .unwrap();
    let meta_eval_ids: Vec<&str> = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["is_meta"] == json!(true))
        .map(|t| t["eval_id"].as_str().unwrap())
        .collect();
    assert_eq!(meta_eval_ids, vec!["pos-eval"]);
}

/// `grade` (emit + `--finalize`): each `run-<k>` subdirectory of a condition
/// cell is graded independently — judge tasks point into the run dir and
/// grading.json lands beside each run.json.
#[test]
fn grade_emits_and_finalizes_per_nested_run_dir() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Fix it.", "expected_output": "debugs",
             "assertions": [{"id": "a1", "type": "llm_judge", "rubric": "Did it debug?"}]}
        ]}),
    );
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    fs::create_dir_all(&iteration_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [
                {"name": "with_skill", "skill_path": skill_md},
                {"name": "without_skill", "skill_path": null},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    let cond_dir = iteration_dir.join("eval-pos-eval").join("with_skill");
    for k in [1, 2] {
        let run_dir = cond_dir.join(format!("run-{k}"));
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(
            run_dir.join("run.json"),
            serde_json::to_string(&json!({
                "eval_id": "pos-eval", "condition": "with_skill", "skill_path": skill_md,
                "prompt": "p", "files": [], "final_message": format!("done in run {k}"),
                "tool_invocations": [], "total_tokens": 100, "duration_ms": 1000,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    grade_cmd(&cwd, &skill_dir, None).assert().success();

    let tasks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("judge-tasks.json")).unwrap())
            .unwrap();
    let a1_response_paths: Vec<&str> = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["assertion_id"] == json!("a1"))
        .map(|t| t["response_path"].as_str().unwrap())
        .collect();
    assert_eq!(a1_response_paths.len(), 2, "one judge task per run");
    for k in [1, 2] {
        let expected = cond_dir
            .join(format!("run-{k}"))
            .join("judge-responses")
            .join("a1.json");
        assert!(
            a1_response_paths
                .iter()
                .any(|p| *p == expected.to_string_lossy()),
            "missing judge task for run-{k}"
        );
        fs::write(
            &expected,
            serde_json::to_string(&json!({"passed": k == 1, "evidence": "e", "confidence": 0.9}))
                .unwrap(),
        )
        .unwrap();
    }

    grade_cmd(&cwd, &skill_dir, None)
        .arg("--finalize")
        .assert()
        .success();

    for (k, expected_rate) in [(1, 1.0), (2, 0.0)] {
        let grading: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(cond_dir.join(format!("run-{k}")).join("grading.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            grading["summary"]["pass_rate"],
            json!(expected_rate),
            "wrong pass rate for run-{k}"
        );
    }
}

/// `grade` (emit): a malformed run.json fails fast with a run-record schema error.
#[test]
fn grade_fails_fast_on_malformed_run_record() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Fix it.", "expected_output": "debugs",
             "assertions": [{"id": "a1", "type": "llm_judge", "rubric": "Did it debug?"}]}
        ]}),
    );

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    fs::create_dir_all(&iteration_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [
                {"name": "with_skill", "skill_path": skill_sub.join("SKILL.md").to_string_lossy()},
                {"name": "without_skill", "skill_path": null},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    for cond in ["with_skill", "without_skill"] {
        let cond_dir = iteration_dir.join("eval-pos-eval").join(cond);
        fs::create_dir_all(&cond_dir).unwrap();
        // Missing required `final_message` and `files` — must be rejected.
        fs::write(
            cond_dir.join("run.json"),
            serde_json::to_string(&json!({
                "eval_id": "pos-eval", "condition": cond, "skill_path": null,
                "prompt": "p", "tool_invocations": [],
            }))
            .unwrap(),
        )
        .unwrap();
    }

    grade_cmd(&cwd, &skill_dir, None)
        .assert()
        .failure()
        .stderr(contains("run-record schema"));
}

/// `grade` (emit): each prompt is written to a file and the inline prompt is
/// dropped from judge-tasks.json (the orchestrator reads it from the file).
#[test]
fn grade_writes_prompt_files_and_drops_inline_prompt() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Fix it.", "expected_output": "debugs",
             "assertions": [{"id": "a1", "type": "llm_judge", "rubric": "Did it debug?"}]}
        ]}),
    );
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    fs::create_dir_all(&iteration_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [
                {"name": "with_skill", "skill_path": skill_md},
                {"name": "without_skill", "skill_path": null},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    for cond in ["with_skill", "without_skill"] {
        let cond_dir = iteration_dir.join("eval-pos-eval").join(cond);
        fs::create_dir_all(&cond_dir).unwrap();
        let skill_path = if cond == "with_skill" {
            json!(skill_md)
        } else {
            json!(null)
        };
        fs::write(
            cond_dir.join("run.json"),
            serde_json::to_string(&json!({
                "eval_id": "pos-eval", "condition": cond, "skill_path": skill_path,
                "prompt": "p", "files": [], "final_message": "done",
                "tool_invocations": [], "total_tokens": 100, "duration_ms": 1000,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    grade_cmd(&cwd, &skill_dir, None).assert().success();

    let tasks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("judge-tasks.json")).unwrap())
            .unwrap();
    let tasks = tasks["tasks"].as_array().unwrap();
    assert!(!tasks.is_empty());
    for t in tasks {
        assert!(
            t.get("dispatch_prompt").is_none(),
            "inline prompt not stripped"
        );
        let prompt_path = t["dispatch_prompt_path"].as_str().unwrap();
        let assertion_id = t["assertion_id"].as_str().unwrap();
        assert!(prompt_path.ends_with(&format!("{assertion_id}.txt")));
        let contents = fs::read_to_string(prompt_path).unwrap();
        assert!(contents.contains(t["response_path"].as_str().unwrap()));
        assert!(contents.contains("Grade only this one assertion"));
        assert!(contents.contains("Do not run eval-magic"));
        assert!(contents.contains("Do not dispatch other judge tasks"));
        assert!(contents.contains("Do not wait for other workers"));
    }
}

/// `grade --finalize`: folds a code-checked meta result + an llm_judge response
/// into a schema-valid grading.json with the right summaries.
#[test]
fn grade_finalize_folds_responses_into_grading() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Fix it.", "expected_output": "debugs",
             "assertions": [{"id": "a1", "type": "llm_judge", "rubric": "Did it debug?"}]}
        ]}),
    );
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();
    let slug = "slow-powers-eval-1-with_skill__mr-review";

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    let cond_dir = iteration_dir.join("eval-pos-eval").join("with_skill");
    fs::create_dir_all(&cond_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [{"name": "with_skill", "skill_path": skill_md, "staged_skill_slug": slug}],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    // Transcript invokes the staged skill → meta is code-checked to passed.
    fs::write(
        cond_dir.join("run.json"),
        serde_json::to_string(&json!({
            "eval_id": "pos-eval", "condition": "with_skill", "skill_path": skill_md,
            "prompt": "p", "files": [], "final_message": "done",
            "tool_invocations": [{"name": "Skill", "args": {"skill": slug}, "ordinal": 0}],
            "total_tokens": 100, "duration_ms": 1000,
        }))
        .unwrap(),
    )
    .unwrap();

    // Emit (writes the code-checked meta response + the a1 judge task).
    grade_cmd(&cwd, &skill_dir, None).assert().success();
    // The orchestrator's a1 verdict.
    fs::write(
        cond_dir.join("judge-responses").join("a1.json"),
        serde_json::to_string(
            &json!({"passed": true, "evidence": "it debugged", "confidence": 0.9}),
        )
        .unwrap(),
    )
    .unwrap();

    // Finalize.
    grade_cmd(&cwd, &skill_dir, None)
        .arg("--finalize")
        .assert()
        .success();

    let grading: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(cond_dir.join("grading.json")).unwrap()).unwrap();
    assert_eq!(grading["summary"]["passed"], json!(1));
    assert_eq!(grading["summary"]["total"], json!(1));
    assert_eq!(grading["summary"]["pass_rate"], json!(1.0));
    assert_eq!(grading["assertion_results"][0]["id"], json!("a1"));
    assert_eq!(grading["assertion_results"][0]["passed"], json!(true));
    assert_eq!(grading["meta_summary"]["skill_invoked"], json!(true));
}
