//! CLI-level command-check finalize behavior.

use crate::helpers::{canonical_root, skill_eval};
use serde_json::json;
use std::fs;

#[test]
fn finalize_folds_command_check_result_into_normal_pass_rate() {
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    fs::write(
        skill_sub.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
    )
    .unwrap();
    fs::write(
        skill_sub.join("evals/evals.json"),
        serde_json::to_string_pretty(&json!({
            "skill_name": "mr-review",
            "codebase": { "path": "." },
            "evals": [{
                "id": "pos-eval",
                "prompt": "Fix it.",
                "expected_output": "tests pass",
                "assertions": [{
                    "id": "held-out-tests",
                    "type": "command_check",
                    "command": "true"
                }]
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join(".eval-magic")
        .join("mr-review")
        .join("iteration-1");
    let cond_dir = iteration_dir.join("eval-pos-eval").join("without_skill");
    fs::create_dir_all(cond_dir.join("command-checks")).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [{"name": "without_skill", "skill_path": null}],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        cond_dir.join("command-checks/held-out-tests.json"),
        serde_json::to_string(&json!({
            "id": "held-out-tests",
            "passed": true,
            "evidence": "exit code matched 0",
            "expected_exit_code": 0,
            "actual_exit_code": 0,
            "stdout": "",
            "stderr": ""
        }))
        .unwrap(),
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["grade", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1", "--finalize"])
        .assert()
        .success();

    let grading: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(cond_dir.join("grading.json")).unwrap()).unwrap();
    assert_eq!(grading["summary"]["pass_rate"], json!(1.0));
    assert_eq!(grading["assertion_results"][0]["id"], "held-out-tests");
    assert_eq!(grading["assertion_results"][0]["grader"], "command_check");
    assert_eq!(grading["assertion_results"][0]["confidence"], json!(1.0));
}

/// An incomplete task carrying an old result may already have received held-out
/// setup or executed a command, so grade must tell the operator not to resume it.
#[test]
fn an_incomplete_task_with_a_cached_command_check_warns_not_to_resume() {
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    fs::write(
        skill_sub.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
    )
    .unwrap();
    fs::write(
        skill_sub.join("evals/evals.json"),
        serde_json::to_string_pretty(&json!({
            "skill_name": "mr-review",
            "codebase": { "path": "." },
            "evals": [{
                "id": "pos-eval",
                "prompt": "Fix it.",
                "expected_output": "tests pass",
                "assertions": [{
                    "id": "held-out-tests",
                    "type": "command_check",
                    "command": "true"
                }]
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join(".eval-magic")
        .join("mr-review")
        .join("iteration-1");
    let cond_dir = iteration_dir.join("eval-pos-eval").join("with_skill");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    fs::create_dir_all(cond_dir.join("command-checks")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [{"name": "with_skill", "skill_path": null}],
            "timestamp": "2026-08-31T00:00:00.000Z",
            "harness": "claude-code"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        iteration_dir.join("dispatch.json"),
        serde_json::to_string(&json!({"tasks": [{
            "eval_id": "pos-eval",
            "condition": "with_skill",
            "eval_root": eval_root,
            "run_record_path": cond_dir.join("run.json"),
        }]}))
        .unwrap(),
    )
    .unwrap();
    // A result produced by a command that is no longer the authored one.
    fs::write(
        cond_dir.join("command-checks/held-out-tests.json"),
        serde_json::to_string(&json!({
            "id": "held-out-tests",
            "passed": true,
            "evidence": "exit code matched 0",
            "expected_exit_code": 0,
            "actual_exit_code": 0,
            "stdout": "",
            "stderr": "",
            "definition_digest": "0000000000000000"
        }))
        .unwrap(),
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["grade", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .success()
        .stdout(predicates::str::contains("1 skipped (missing run.json)"))
        .stderr(predicates::str::contains("pos-eval/with_skill"))
        .stderr(predicates::str::contains("may already be contaminated"))
        .stderr(predicates::str::contains("must not be resumed"))
        .stderr(predicates::str::contains("fresh iteration"));
}
