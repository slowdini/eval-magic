use super::*;
use serde_json::json;

fn write_grading_json_in(run_dir: &std::path::Path, grading: serde_json::Value) {
    fs::create_dir_all(run_dir).unwrap();
    fs::write(
        run_dir.join("grading.json"),
        serde_json::to_string(&grading).unwrap(),
    )
    .unwrap();
}

/// `aggregate`: substantive assertion results are counted separately for each
/// eval, assertion, and condition, while framework meta-results stay out of the
/// effectiveness report.
#[test]
fn aggregate_rolls_up_substantive_assertions_by_eval_and_condition() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);

    let runs = [
        (
            "with_skill",
            1,
            json!([
                {"id": "behavior", "passed": true, "evidence": "yes", "grader": "llm_judge"},
                {"id": "held-out", "passed": false, "evidence": "exit 1", "grader": "command_check"}
            ]),
        ),
        (
            "with_skill",
            2,
            json!([
                {"id": "behavior", "passed": true, "evidence": "yes", "grader": "llm_judge"},
                {"id": "observed-tool", "passed": true, "evidence": "seen", "grader": "transcript_check"}
            ]),
        ),
        (
            "without_skill",
            1,
            json!([
                {"id": "behavior", "passed": false, "evidence": "no", "grader": "llm_judge"},
                {"id": "held-out", "passed": true, "evidence": "exit 0", "grader": "command_check"}
            ]),
        ),
        (
            "without_skill",
            2,
            json!([
                {"id": "behavior", "passed": true, "evidence": "yes", "grader": "llm_judge"},
                {"id": "held-out", "passed": false, "evidence": "exit 1", "grader": "command_check"},
                {"id": "focused-change", "passed": false, "evidence": "too large", "grader": "diff_scope"}
            ]),
        ),
    ];

    for (condition, run_index, assertion_results) in runs {
        let passed = assertion_results
            .as_array()
            .unwrap()
            .iter()
            .filter(|result| result["passed"] == true)
            .count();
        let total = assertion_results.as_array().unwrap().len();
        let run_dir = iteration_dir
            .join("eval-e1")
            .join(condition)
            .join(format!("run-{run_index}"));
        write_grading_json_in(
            &run_dir,
            json!({
                "assertion_results": assertion_results,
                "summary": {
                    "passed": passed,
                    "failed": total - passed,
                    "total": total,
                    "pass_rate": passed as f64 / total as f64
                },
                "meta_results": [{
                    "id": "__skill_invoked",
                    "passed": true,
                    "evidence": "seen",
                    "grader": "transcript_check"
                }],
                "meta_summary": {
                    "passed": 1,
                    "failed": 0,
                    "total": 1,
                    "skill_invoked": true
                }
            }),
        );
        write_timing_in(&run_dir, json!({"total_tokens": 1000, "duration_ms": 100}));
    }

    agg_cmd(&cwd, &skill_dir).assert().success();

    let benchmark = read_benchmark(&iteration_dir);
    assert_eq!(
        benchmark["assertions"],
        json!({
            "e1": {
                "behavior": {
                    "with_skill": {"passed": 2, "n": 2},
                    "without_skill": {"passed": 1, "n": 2}
                },
                "held-out": {
                    "with_skill": {"passed": 0, "n": 1},
                    "without_skill": {"passed": 1, "n": 2}
                },
                "focused-change": {
                    "without_skill": {"passed": 0, "n": 1}
                },
                "observed-tool": {
                    "with_skill": {"passed": 1, "n": 1}
                }
            }
        })
    );
    assert_eq!(benchmark["delta"]["pass_rate"], 0.333);
    assert!(
        !benchmark["assertions"]
            .to_string()
            .contains("__skill_invoked")
    );
}
