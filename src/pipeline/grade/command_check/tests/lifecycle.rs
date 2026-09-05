//! Persistence, overwrite, isolation, and declaration-order behavior.

use super::*;

#[test]
fn persisted_results_are_reused_and_overwrite_reruns_in_declaration_order() {
    let root = tempfile::TempDir::new().unwrap();
    let skill_dir = root.path().join("skill");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    fs::create_dir_all(skill_dir.join("evals/holdout")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    fs::write(skill_dir.join("evals/holdout/secret.txt"), "held out").unwrap();
    write_dispatch(&iteration_dir, &eval_root, false);
    assert!(!eval_root.join("holdout/secret.txt").exists());

    let first = grade_frozen(&iteration_dir, evals(&append_command()), &skill_dir, false);
    assert_eq!(first.executed, 1);
    assert_eq!(first.reused, 0);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "x"
    );
    assert_eq!(
        fs::read_to_string(eval_root.join("holdout/secret.txt")).unwrap(),
        "held out"
    );
    let result_path = iteration_dir.join("eval-e1/with_skill/command-checks/check.json");
    assert!(result_path.exists());
    assert!(
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&result_path).unwrap())
            .unwrap()
            .get("run_record_digest")
            .is_some(),
        "a persisted command check identifies the run record it graded"
    );

    let reused = grade_frozen(&iteration_dir, evals(&append_command()), &skill_dir, false);
    assert_eq!(reused.executed, 0);
    assert_eq!(reused.reused, 1);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "x"
    );

    let overwritten = grade_frozen(&iteration_dir, evals(&append_command()), &skill_dir, true);
    assert_eq!(overwritten.executed, 1);
    assert_eq!(overwritten.reused, 0);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "xx"
    );
}

#[test]
fn persisted_matrix_results_are_schema_gated_and_reused() {
    let root = tempfile::TempDir::new().unwrap();
    let skill_dir = root.path().join("skill");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    fs::create_dir_all(skill_dir.join("evals/holdout")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    fs::write(skill_dir.join("evals/holdout/secret.txt"), "held out").unwrap();
    write_dispatch(&iteration_dir, &eval_root, false);

    let mut config = evals(&append_command());
    let crate::core::Assertion::CommandCheck(check) = config.evals[0]
        .assertions
        .as_mut()
        .unwrap()
        .first_mut()
        .unwrap()
    else {
        panic!("expected command_check");
    };
    check.matrix = Some(std::collections::BTreeMap::from([(
        "TZ".into(),
        vec!["UTC".into(), "Europe/Berlin".into()],
    )]));

    let first = grade_frozen(&iteration_dir, config.clone(), &skill_dir, false);
    assert_eq!(first.executed, 1);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "xx"
    );
    let result_path = iteration_dir.join("eval-e1/with_skill/command-checks/check.json");
    let result: CommandCheckResult =
        serde_json::from_str(&fs::read_to_string(&result_path).unwrap()).unwrap();
    assert_eq!(result.cells.as_ref().unwrap().len(), 2);

    let reused = grade_frozen(&iteration_dir, config.clone(), &skill_dir, false);
    assert_eq!(reused.executed, 0);
    assert_eq!(reused.reused, 1);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "xx"
    );

    let overwritten = grade_frozen(&iteration_dir, config.clone(), &skill_dir, true);
    assert_eq!(overwritten.executed, 1);
    assert_eq!(overwritten.reused, 0);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "xxxx"
    );
}

#[test]
fn shared_eval_root_is_rejected_with_fresh_iteration_guidance() {
    let root = tempfile::TempDir::new().unwrap();
    let skill_dir = root.path().join("skill");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    fs::create_dir_all(skill_dir.join("evals/holdout")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    fs::write(skill_dir.join("evals/holdout/secret.txt"), "held out").unwrap();
    write_dispatch(&iteration_dir, &eval_root, true);

    let error = grade_command_checks(
        &iteration_dir,
        &GradingInstrument::frozen(evals(&exit_command(0)), &skill_dir),
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("shares eval_root"), "{error}");
    assert!(error.contains("fresh iteration"), "{error}");
}

#[test]
fn multiple_checks_execute_in_declaration_order_against_one_env() {
    let root = tempfile::TempDir::new().unwrap();
    let skill_dir = root.path().join("skill");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    fs::create_dir_all(skill_dir.join("evals")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    write_dispatch(&iteration_dir, &eval_root, false);
    let evals: EvalsConfig = serde_json::from_value(json!({
        "skill_name": "demo",
        "evals": [{
            "id": "e1",
            "prompt": "p",
            "expected_output": "o",
            "assertions": [
                {
                    "id": "first",
                    "type": "command_check",
                    "command": fixture(&["--text", "ready", "--write", "state.txt"])
                },
                {
                    "id": "second",
                    "type": "command_check",
                    "command": fixture(&["--require-file-text", "state.txt", "ready"])
                }
            ]
        }]
    }))
    .unwrap();

    let summary = grade_frozen(&iteration_dir, evals, &skill_dir, false);
    assert_eq!(summary.executed, 2);
    for id in ["first", "second"] {
        let result: CommandCheckResult = serde_json::from_str(
            &fs::read_to_string(
                iteration_dir.join(format!("eval-e1/with_skill/command-checks/{id}.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(result.passed, "{id}: {}", result.evidence);
    }
}
