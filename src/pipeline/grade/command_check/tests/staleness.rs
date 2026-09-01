//! Command checks read and reused after the assertion set has moved on.

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

/// Cached results are keyed by assertion id, so an edited `command_check` under
/// an unchanged id would silently be reported from the old command. Now that
/// assertions can be edited after the run (#295), say so instead.
#[test]
fn a_reused_result_whose_check_changed_is_reported_as_stale() {
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
    let edited = grade_frozen(&iteration_dir, evals(&exit_command(3)), &skill_dir, false);

    assert_eq!(edited.reused, 1, "the persisted result is still reused");
    let warning = edited.warnings.join("\n");
    assert!(
        warning.contains("check"),
        "the stale check is named: {warning}"
    );
    assert!(
        warning.contains("--overwrite"),
        "and the way to re-execute it: {warning}"
    );

    // An unedited check stays quiet.
    let unchanged = grade_frozen(&iteration_dir, evals(&exit_command(0)), &skill_dir, false);
    assert_eq!(unchanged.reused, 1);
    assert!(unchanged.warnings.is_empty(), "{:?}", unchanged.warnings);
}
