//! Which `evals.json` a grading measured against, and what it says about it.

use super::*;

/// Write the copy an iteration froze at run time: `.skills/<skill>/`, holding
/// the treatment and the eval definitions as they stood when the run started.
fn write_frozen_copy(
    iteration_dir: &std::path::Path,
    skill: &str,
    skill_md: &str,
    evals: &serde_json::Value,
) -> std::path::PathBuf {
    let copy = iteration_dir.join(".skills").join(skill);
    fs::create_dir_all(copy.join("evals")).unwrap();
    fs::write(copy.join("SKILL.md"), skill_md).unwrap();
    fs::write(
        copy.join("evals").join("evals.json"),
        serde_json::to_string_pretty(&with_default_codebase(evals)).unwrap(),
    )
    .unwrap();
    copy
}

/// #295: `eval-magic docs judging` prescribes running both conditions first and
/// authoring assertions from the paired evidence. Grading the frozen copy made
/// that a silent no-op — `Judge tasks: 0`, no error, no warning. The assertions
/// are the measuring instrument, not the treatment, so they come from the live
/// file and grading says which file it read them from.
#[test]
fn assertions_authored_after_the_run_are_what_grade_measures() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    let skill_md_body = "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n";
    let eval = json!({
        "id": "implement-feature",
        "prompt": "Implement the feature.",
        "expected_output": "A working feature.",
    });

    // The live file carries the assertions written from the run's evidence.
    let mut authored = eval.clone();
    authored["assertions"] = json!([
        {"id": "quality", "type": "llm_judge", "rubric": "Is the feature well tested?"}
    ]);
    write_skill(
        &skill_sub,
        skill_md_body,
        &json!({"skill_name": "mr-review", "evals": [authored]}),
    );

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join(".eval-magic")
        .join("mr-review")
        .join("iteration-1");
    // The frozen copy has none: it was written before the run produced evidence.
    let frozen = write_frozen_copy(
        &iteration_dir,
        "mr-review",
        skill_md_body,
        &json!({"skill_name": "mr-review", "evals": [eval]}),
    );
    let staged_skill_md = frozen.join("SKILL.md").to_string_lossy().into_owned();

    let cond_dir = iteration_dir
        .join("eval-implement-feature")
        .join("with_skill");
    fs::create_dir_all(&cond_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [{"name": "with_skill", "skill_path": staged_skill_md}],
            "timestamp": "2026-08-31T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        cond_dir.join("run.json"),
        serde_json::to_string(&json!({
            "eval_id": "implement-feature", "condition": "with_skill",
            "skill_path": staged_skill_md,
            "prompt": "p", "files": [], "final_message": "Implemented and tested.",
            "tool_invocations": [], "total_tokens": 100, "duration_ms": 1000,
        }))
        .unwrap(),
    )
    .unwrap();

    let live_evals = skill_sub.join("evals").join("evals.json");
    grade_cmd(&cwd, &skill_dir, None)
        .assert()
        .success()
        .stdout(contains(format!(
            "Assertions: {}",
            live_evals.to_string_lossy()
        )))
        .stdout(contains("refreshed"))
        .stdout(contains("implement-feature"));

    let tasks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("judge-tasks.json")).unwrap())
            .unwrap();
    let authored_tasks: Vec<&str> = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|task| task["is_meta"] == json!(false))
        .map(|task| task["assertion_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        authored_tasks,
        vec!["quality"],
        "the assertion added after the run has to reach the judge"
    );

    // The grading records which file it measured with, so a report is not read
    // against an assertion set nobody can identify later.
    let responses = cond_dir.join("judge-responses");
    for stem in ["quality", "__skill_invoked"] {
        fs::write(
            responses.join(format!("{stem}.json")),
            serde_json::to_string(&json!({
                "passed": true, "evidence": "tests added", "confidence": 0.9
            }))
            .unwrap(),
        )
        .unwrap();
    }

    grade_cmd(&cwd, &skill_dir, None)
        .arg("--finalize")
        .assert()
        .success();

    let grading: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(cond_dir.join("grading.json")).unwrap()).unwrap();
    assert_eq!(
        grading["assertion_source"]["path"],
        json!(live_evals.to_string_lossy())
    );
    assert_eq!(grading["assertion_source"]["refreshed"], json!(true));
    assert!(
        grading["assertion_source"]["digest"]
            .as_str()
            .is_some_and(|digest| !digest.is_empty()),
        "the recorded source carries a digest of the graded assertion set"
    );
}

/// Judge verdicts are cached by assertion id, so a reworded rubric under an
/// unchanged id would be reported from the verdict on the old rubric. Now that
/// assertions are expected to be edited between runs (#295), say so.
#[test]
fn a_reworded_rubric_reports_the_verdict_it_would_reuse_as_stale() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    let evals = |rubric: &str| {
        json!({"skill_name": "mr-review", "evals": [{
            "id": "pos-eval", "prompt": "Review this MR.", "expected_output": "A review.",
            "assertions": [{"id": "quality", "type": "llm_judge", "rubric": rubric}]
        }]})
    };
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &evals("Did it review systematically?"),
    );
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join(".eval-magic")
        .join("mr-review")
        .join("iteration-1");
    let cond_dir = iteration_dir.join("eval-pos-eval").join("with_skill");
    fs::create_dir_all(&cond_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [{"name": "with_skill", "skill_path": skill_md}],
            "timestamp": "2026-08-31T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        cond_dir.join("run.json"),
        serde_json::to_string(&json!({
            "eval_id": "pos-eval", "condition": "with_skill", "skill_path": skill_md,
            "prompt": "p", "files": [], "final_message": "Reviewed.",
            "tool_invocations": [], "total_tokens": 100, "duration_ms": 1000,
        }))
        .unwrap(),
    )
    .unwrap();

    grade_cmd(&cwd, &skill_dir, None).assert().success();

    // A judge answered the rubric as it stood.
    fs::write(
        cond_dir.join("judge-responses").join("quality.json"),
        serde_json::to_string(&json!({
            "passed": true, "evidence": "systematic", "confidence": 0.9
        }))
        .unwrap(),
    )
    .unwrap();

    // Re-grading an unedited rubric has nothing to report.
    grade_cmd(&cwd, &skill_dir, None)
        .assert()
        .success()
        .stderr(contains("quality").not());

    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &evals("Did it rank findings by severity?"),
    );

    grade_cmd(&cwd, &skill_dir, None)
        .assert()
        .success()
        .stderr(contains("quality"))
        .stderr(contains("--overwrite"));
}
