//! Model-selection behavior for emitted judge tasks.

use crate::helpers::{canonical_root, skill_eval};
use assert_cmd::Command;
use std::fs;

fn write_skill(skill_sub: &std::path::Path, skill_md: &str, evals: &serde_json::Value) {
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    fs::write(skill_sub.join("SKILL.md"), skill_md).unwrap();
    fs::write(
        skill_sub.join("evals").join("evals.json"),
        serde_json::to_string_pretty(evals).unwrap(),
    )
    .unwrap();
}

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

/// `grade` (emit): run-level `conditions.judge_model` is the default for judge
/// tasks, but a per-assertion `model` remains the most specific override.
#[test]
fn grade_defaults_judge_tasks_to_recorded_judge_model() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nUse the MERGE-RISK-LADDER before writing the final review.",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Review this MR.", "expected_output": "Agent reviews systematically.",
             "assertions": [
                {"id": "defaulted", "type": "llm_judge", "rubric": "Did it review systematically?"},
                {"id": "specific", "type": "llm_judge", "rubric": "Did it cite evidence?", "model": "judge-specific-model"}
             ]}
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
            "judge_model": "run-default-judge",
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

    let assert = grade_cmd(&cwd, &skill_dir, Some("codex"))
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("codex exec"));
    assert!(stdout.contains("-m \"$model\""));

    let tasks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("judge-tasks.json")).unwrap())
            .unwrap();
    let model_for = |id: &str| {
        tasks["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["assertion_id"] == json!(id))
            .unwrap()["model"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(model_for("defaulted"), "run-default-judge");
    assert_eq!(model_for("specific"), "judge-specific-model");
    assert_eq!(model_for("__skill_invoked"), "run-default-judge");
}
