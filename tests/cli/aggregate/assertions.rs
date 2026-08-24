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

fn sampled_grading(passed: u32, total: u32) -> serde_json::Value {
    let proportion = f64::from(passed) / f64::from(total);
    let judge_samples: Vec<serde_json::Value> = (1..=total)
        .map(|sample_index| {
            let sample_passed = sample_index <= passed;
            json!({
                "sample_index": sample_index,
                "passed": sample_passed,
                "evidence": format!("sample {sample_index}"),
                "confidence": 0.8
            })
        })
        .collect();
    let pass_power_k = proportion.powf(f64::from(total));
    json!({
        "assertion_results": [{
            "id": "quality",
            "grader": "llm_judge",
            "votes": {
                "passed": passed,
                "failed": total - passed,
                "total": total,
                "proportion": proportion,
                "pass_power_k": pass_power_k
            },
            "judge_samples": judge_samples
        }],
        "summary": {
            "total": 1,
            "pass_rate": proportion,
            "vote_proportion": proportion,
            "pass_power_k": pass_power_k
        }
    })
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

#[test]
fn aggregate_surfaces_sampled_votes_and_pass_power_k_by_condition() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);

    for (condition, run_index, passed) in [
        ("with_skill", 1, 3),
        ("with_skill", 2, 4),
        ("without_skill", 1, 2),
        ("without_skill", 2, 2),
    ] {
        let run_dir = iteration_dir
            .join("eval-e1")
            .join(condition)
            .join(format!("run-{run_index}"));
        write_grading_json_in(&run_dir, sampled_grading(passed, 4));
        write_timing_in(&run_dir, json!({"total_tokens": 1000, "duration_ms": 100}));
    }

    agg_cmd(&cwd, &skill_dir).assert().success();

    let benchmark = read_benchmark(&iteration_dir);
    assert_eq!(
        benchmark["assertions"]["e1"]["quality"]["with_skill"],
        json!({
            "votes": {"passed": 7, "failed": 1, "total": 8, "proportion": 0.875},
            "samples_per_run": 4,
            "run_count": 2,
            "pass_power_k": 0.586181640625
        })
    );
    assert_eq!(
        benchmark["assertions"]["e1"]["quality"]["without_skill"],
        json!({
            "votes": {"passed": 4, "failed": 4, "total": 8, "proportion": 0.5},
            "samples_per_run": 4,
            "run_count": 2,
            "pass_power_k": 0.0625
        })
    );
    assert_eq!(
        benchmark["run_summary"]["with_skill"]["vote_proportion"],
        json!({"mean": 0.875, "stddev": 0.125, "n": 2})
    );
    assert_eq!(
        benchmark["run_summary"]["with_skill"]["pass_power_k"],
        json!({"mean": 0.658203, "stddev": 0.341797, "n": 2})
    );
    assert_eq!(benchmark["delta"]["vote_proportion"], 0.375);
    assert_eq!(benchmark["delta"]["pass_power_k"], 0.595703);
}
