//! Command-check cache eligibility across definition, run, and completion changes.

use super::*;

/// #295: a `command_check` written after the run names a held-out setup file
/// that exists only in the live skill tree, so setup files have to follow the
/// assertions that reference them rather than the copy the run froze.
#[test]
fn setup_files_for_an_assertion_added_after_the_run_come_from_the_live_tree() {
    use crate::pipeline::grade::instrument::resolve_grading_instrument;

    let root = tempfile::TempDir::new().unwrap();
    let frozen_dir = root.path().join("frozen");
    let live_dir = root.path().join("live");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    fs::create_dir_all(&eval_root).unwrap();
    write_dispatch(&iteration_dir, &eval_root, false);

    let base = json!({
        "skill_name": "demo",
        "codebase": { "path": "." },
        "evals": [{ "id": "e1", "prompt": "p", "expected_output": "o" }],
    });
    fs::create_dir_all(frozen_dir.join("evals")).unwrap();
    fs::write(
        frozen_dir.join("evals/evals.json"),
        serde_json::to_vec(&base).unwrap(),
    )
    .unwrap();

    // The assertion and its held-out file exist only in the live tree.
    let mut authored = base.clone();
    authored["evals"][0]["assertions"] = json!([{
        "id": "check",
        "type": "command_check",
        "setup_files": ["holdout/secret.txt"],
        "command": append_command(),
    }]);
    fs::create_dir_all(live_dir.join("evals/holdout")).unwrap();
    fs::write(
        live_dir.join("evals/evals.json"),
        serde_json::to_vec(&authored).unwrap(),
    )
    .unwrap();
    fs::write(live_dir.join("evals/holdout/secret.txt"), "held out").unwrap();

    let instrument = resolve_grading_instrument(&frozen_dir, &live_dir).unwrap();
    let summary = grade_command_checks(&iteration_dir, &instrument, false).unwrap();

    assert_eq!(summary.executed, 1);
    assert_eq!(
        fs::read_to_string(eval_root.join("holdout/secret.txt")).unwrap(),
        "held out"
    );
}

/// A cached result is reusable only for the authored check that produced it.
#[test]
fn a_changed_command_check_is_executed_instead_of_reusing_the_old_result() {
    let root = tempfile::TempDir::new().unwrap();
    let skill_dir = root.path().join("skill");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    fs::create_dir_all(skill_dir.join("evals/holdout")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    fs::write(skill_dir.join("evals/holdout/secret.txt"), "held out").unwrap();
    write_dispatch(&iteration_dir, &eval_root, false);

    let first = grade_frozen(&iteration_dir, evals(&exit_command(0)), &skill_dir, false);
    assert_eq!(first.executed, 1);
    assert!(first.warnings.is_empty());

    // Same assertion id, different command.
    let edited = grade_frozen(&iteration_dir, evals(&append_command()), &skill_dir, false);

    assert_eq!(edited.executed, 1);
    assert_eq!(edited.reused, 0);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "x"
    );

    let unchanged = grade_frozen(&iteration_dir, evals(&append_command()), &skill_dir, false);
    assert_eq!(unchanged.reused, 1);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "x"
    );
}

#[test]
fn a_result_for_another_run_record_is_executed_once_then_reused() {
    let root = tempfile::TempDir::new().unwrap();
    let skill_dir = root.path().join("skill");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    let run_record = iteration_dir.join("eval-e1/with_skill/run.json");
    fs::create_dir_all(skill_dir.join("evals/holdout")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    fs::write(skill_dir.join("evals/holdout/secret.txt"), "held out").unwrap();
    write_dispatch(&iteration_dir, &eval_root, false);

    let first = grade_frozen(&iteration_dir, evals(&append_command()), &skill_dir, false);
    assert_eq!(first.executed, 1);
    let first_result =
        fs::read_to_string(iteration_dir.join("eval-e1/with_skill/command-checks/check.json"))
            .unwrap();
    let first_digest =
        serde_json::from_str::<serde_json::Value>(&first_result).unwrap()["run_record_digest"]
            .as_str()
            .unwrap()
            .to_string();

    fs::write(&run_record, r#"{"run":"replacement"}"#).unwrap();
    let replacement = grade_frozen(&iteration_dir, evals(&append_command()), &skill_dir, false);
    assert_eq!(replacement.executed, 1);
    assert_eq!(replacement.reused, 0);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "xx"
    );
    let replacement_result =
        fs::read_to_string(iteration_dir.join("eval-e1/with_skill/command-checks/check.json"))
            .unwrap();
    let replacement_digest = serde_json::from_str::<serde_json::Value>(&replacement_result)
        .unwrap()["run_record_digest"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(replacement_digest, first_digest);

    let reused = grade_frozen(&iteration_dir, evals(&append_command()), &skill_dir, false);
    assert_eq!(reused.executed, 0);
    assert_eq!(reused.reused, 1);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "xx"
    );
}

#[test]
fn a_legacy_result_without_run_identity_is_executed_once_then_reused() {
    let root = tempfile::TempDir::new().unwrap();
    let skill_dir = root.path().join("skill");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    fs::create_dir_all(skill_dir.join("evals/holdout")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    fs::write(skill_dir.join("evals/holdout/secret.txt"), "held out").unwrap();
    write_dispatch(&iteration_dir, &eval_root, false);

    let legacy = grade_frozen(&iteration_dir, evals(&append_command()), &skill_dir, false);
    assert_eq!(legacy.executed, 1);
    let result_path = iteration_dir.join("eval-e1/with_skill/command-checks/check.json");
    let mut legacy_value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&result_path).unwrap()).unwrap();
    legacy_value
        .as_object_mut()
        .unwrap()
        .remove("run_record_digest");
    fs::write(&result_path, serde_json::to_vec(&legacy_value).unwrap()).unwrap();

    let refreshed = grade_frozen(&iteration_dir, evals(&append_command()), &skill_dir, false);
    assert_eq!(refreshed.executed, 1);
    assert_eq!(refreshed.reused, 0);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "xx"
    );

    let reused = grade_frozen(&iteration_dir, evals(&append_command()), &skill_dir, false);
    assert_eq!(reused.executed, 0);
    assert_eq!(reused.reused, 1);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "xx"
    );
}

#[test]
fn an_incomplete_task_with_a_cached_result_is_never_touched_and_warns() {
    let root = tempfile::TempDir::new().unwrap();
    let skill_dir = root.path().join("skill");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    let run_record = iteration_dir.join("eval-e1/with_skill/run.json");
    let result_path = iteration_dir.join("eval-e1/with_skill/command-checks/check.json");
    fs::create_dir_all(skill_dir.join("evals/holdout")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    fs::write(skill_dir.join("evals/holdout/secret.txt"), "held out").unwrap();
    write_dispatch(&iteration_dir, &eval_root, false);
    fs::remove_file(run_record).unwrap();
    fs::create_dir_all(result_path.parent().unwrap()).unwrap();
    let legacy = json!({
        "id": "check",
        "passed": true,
        "evidence": "old result",
        "expected_exit_code": 0,
        "actual_exit_code": 0,
        "stdout": "",
        "stderr": ""
    })
    .to_string();
    fs::write(&result_path, &legacy).unwrap();

    for overwrite in [false, true] {
        let summary = grade_frozen(
            &iteration_dir,
            evals(&append_command()),
            &skill_dir,
            overwrite,
        );
        assert_eq!(summary.skipped_incomplete, 1);
        assert_eq!(summary.executed, 0);
        assert_eq!(summary.reused, 0);
        let warning = summary.warnings.join("\n");
        assert!(warning.contains("e1/with_skill"), "{warning}");
        assert!(warning.contains("may already be contaminated"), "{warning}");
        assert!(warning.contains("must not be resumed"), "{warning}");
        assert!(warning.contains("fresh iteration"), "{warning}");
        assert_eq!(fs::read_to_string(&result_path).unwrap(), legacy);
        assert!(!eval_root.join("holdout/secret.txt").exists());
        assert!(!eval_root.join("command-runs.txt").exists());
    }
}
