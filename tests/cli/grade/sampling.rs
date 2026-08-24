//! Multi-sample judge-task emission and finalization.

use super::*;

#[test]
fn sampled_judge_paths_reject_colliding_authored_assertion_ids() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [{
            "id": "sampled", "prompt": "Review it.", "expected_output": "a review",
            "skill_should_trigger": false,
            "assertions": [
                {"id": "quality", "type": "llm_judge", "rubric": "Good?", "samples": 2},
                {"id": "quality__sample-1", "type": "llm_judge", "rubric": "Clear?"}
            ]
        }]}),
    );

    let cwd = root.join("work");
    let iteration_dir = cwd.join(".eval-magic/mr-review/iteration-1");
    let cell = iteration_dir.join("eval-sampled/with_skill");
    fs::create_dir_all(&cell).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [{"name": "with_skill", "skill_path": null}],
            "timestamp": "2026-08-23T00:00:00Z",
            "harness": "codex"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        cell.join("run.json"),
        serde_json::to_string(&json!({
            "eval_id": "sampled", "condition": "with_skill", "skill_path": null,
            "prompt": "Review it.", "files": [], "final_message": "Done.",
            "tool_invocations": [], "total_tokens": 10, "duration_ms": 20
        }))
        .unwrap(),
    )
    .unwrap();

    grade_cmd(&cwd, &skill_dir, Some("codex"))
        .assert()
        .failure()
        .stderr(contains("judge task filename collision"))
        .stderr(contains("quality__sample-1"))
        .stderr(contains("quality"));
}

#[test]
fn grade_emits_resolved_judge_samples_with_unique_paths_and_shared_evidence() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [{
            "id": "sampled", "prompt": "Review it.", "expected_output": "a review",
            "skill_should_trigger": false,
            "assertions": [
                {"id": "run-default", "type": "llm_judge", "rubric": "Good?"},
                {"id": "explicit-single", "type": "llm_judge", "rubric": "Clear?", "samples": 1},
                {"id": "explicit-three", "type": "llm_judge", "rubric": "Safe?", "samples": 3}
            ]
        }]}),
    );

    let cwd = root.join("work");
    let iteration_dir = cwd.join(".eval-magic/mr-review/iteration-1");
    let cell = iteration_dir.join("eval-sampled/with_skill");
    fs::create_dir_all(&cell).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [{"name": "with_skill", "skill_path": null}],
            "timestamp": "2026-08-23T00:00:00Z",
            "harness": "codex",
            "judge_samples": 2
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        cell.join("run.json"),
        serde_json::to_string(&json!({
            "eval_id": "sampled", "condition": "with_skill", "skill_path": null,
            "prompt": "Review it.", "files": [], "final_message": "Done.",
            "tool_invocations": [], "total_tokens": 10, "duration_ms": 20
        }))
        .unwrap(),
    )
    .unwrap();

    grade_cmd(&cwd, &skill_dir, Some("codex"))
        .assert()
        .success()
        .stdout(contains(
            "Judge tasks: 6 (0 skill-invocation meta-judge(s))",
        ));

    let artifact: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("judge-tasks.json")).unwrap())
            .unwrap();
    let tasks = artifact["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 6);

    let single = tasks
        .iter()
        .find(|task| task["assertion_id"] == "explicit-single")
        .unwrap();
    assert!(single.get("sample_index").is_none());
    assert!(single.get("sample_count").is_none());
    assert!(
        single["response_path"]
            .as_str()
            .unwrap()
            .ends_with("explicit-single.json")
    );
    assert!(
        single["dispatch_prompt_path"]
            .as_str()
            .unwrap()
            .ends_with("explicit-single.txt")
    );

    for (assertion, expected) in [("run-default", 2_u64), ("explicit-three", 3_u64)] {
        let sampled: Vec<&serde_json::Value> = tasks
            .iter()
            .filter(|task| task["assertion_id"] == assertion)
            .collect();
        assert_eq!(sampled.len() as u64, expected);
        for (offset, task) in sampled.into_iter().enumerate() {
            let index = offset as u64 + 1;
            assert_eq!(task["sample_index"], json!(index));
            assert_eq!(task["sample_count"], json!(expected));
            assert!(
                task["response_path"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!("{assertion}__sample-{index}.json"))
            );
            assert!(
                task["dispatch_prompt_path"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!("{assertion}__sample-{index}.txt"))
            );
        }
    }

    let evidence_paths: std::collections::HashSet<&str> = tasks
        .iter()
        .map(|task| task["evidence_bundle"]["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        evidence_paths.len(),
        1,
        "every sample reuses one run bundle"
    );
}

#[test]
fn finalize_keeps_each_sample_and_counts_a_missing_response_as_one_fail_vote() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [{
            "id": "sampled", "prompt": "Review it.", "expected_output": "a review",
            "skill_should_trigger": false,
            "assertions": [
                {"id": "quality", "type": "llm_judge", "rubric": "Is it good?", "samples": 4},
                {"id": "single", "type": "llm_judge", "rubric": "Is it clear?", "samples": 1}
            ]
        }]}),
    );

    let cwd = root.join("work");
    let iteration_dir = cwd.join(".eval-magic/mr-review/iteration-1");
    let cell = iteration_dir.join("eval-sampled/without_skill");
    fs::create_dir_all(&cell).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [{"name": "without_skill", "skill_path": null}],
            "timestamp": "2026-08-23T00:00:00Z",
            "harness": "codex"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        cell.join("run.json"),
        serde_json::to_string(&json!({
            "eval_id": "sampled", "condition": "without_skill", "skill_path": null,
            "prompt": "Review it.", "files": [], "final_message": "Done.",
            "tool_invocations": [], "total_tokens": 10, "duration_ms": 20
        }))
        .unwrap(),
    )
    .unwrap();

    grade_cmd(&cwd, &skill_dir, Some("codex"))
        .assert()
        .success();
    let responses = cell.join("judge-responses");
    for (index, passed) in [(1, true), (2, false), (3, true)] {
        fs::write(
            responses.join(format!("quality__sample-{index}.json")),
            serde_json::to_string(&json!({
                "passed": passed,
                "evidence": format!("sample {index} evidence"),
                "confidence": 0.8
            }))
            .unwrap(),
        )
        .unwrap();
    }
    fs::write(
        responses.join("single.json"),
        serde_json::to_string(&json!({
            "passed": true,
            "evidence": "the result is clear",
            "confidence": 0.9
        }))
        .unwrap(),
    )
    .unwrap();

    let finalized = grade_cmd(&cwd, &skill_dir, Some("codex"))
        .arg("--finalize")
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&finalized.get_output().stderr);
    assert!(
        stderr.contains("quality__sample-4.json") && stderr.contains("sample will be FAIL"),
        "missing sample warning was: {stderr}"
    );

    let grading: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(cell.join("grading.json")).unwrap()).unwrap();
    let result = &grading["assertion_results"][0];
    assert_eq!(result["id"], "quality");
    assert_eq!(result["grader"], "llm_judge");
    assert!(result.get("passed").is_none());
    assert!(result.get("evidence").is_none());
    assert!(result.get("confidence").is_none());
    assert_eq!(
        result["votes"],
        json!({
            "passed": 2,
            "failed": 2,
            "total": 4,
            "proportion": 0.5,
            "pass_power_k": 0.0625
        })
    );
    let samples = result["judge_samples"].as_array().unwrap();
    assert_eq!(samples.len(), 4);
    assert_eq!(samples[0]["sample_index"], 1);
    assert_eq!(samples[0]["evidence"], "sample 1 evidence");
    assert_eq!(samples[3]["sample_index"], 4);
    assert_eq!(samples[3]["passed"], false);
    assert_eq!(samples[3]["confidence"], 0.0);
    assert!(
        samples[3]["evidence"]
            .as_str()
            .unwrap()
            .contains("quality__sample-4.json")
    );
    assert_eq!(grading["assertion_results"][1]["id"], "single");
    assert_eq!(grading["assertion_results"][1]["passed"], true);
    assert!(grading["assertion_results"][1].get("votes").is_none());
    assert_eq!(
        grading["summary"],
        json!({
            "total": 2,
            "pass_rate": 0.75,
            "vote_proportion": 0.75,
            "pass_power_k": 0.53125
        })
    );
}
