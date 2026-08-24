//! Always-on final-environment diff measurement.

use crate::helpers::*;
use serde_json::json;
use std::fs;
use std::path::Path;

#[test]
fn ingest_writes_diff_scope_for_every_run_without_an_assertion() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "edit", "prompt": "fix source.txt", "expected_output": "fixed",
          "skill_should_trigger": false, "files": ["source.txt"] } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    fs::write(skill_dir.join("mr-review/evals/source.txt"), "old\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--no-guard"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        let eval_root = Path::new(task["eval_root"].as_str().unwrap());
        let outputs_dir = Path::new(task["outputs_dir"].as_str().unwrap());
        fs::write(outputs_dir.join("final-message.md"), "done").unwrap();
        if task["condition"] == "with_skill" {
            fs::write(eval_root.join("source.txt"), "new\n").unwrap();
            fs::write(eval_root.join("notes.txt"), "one\n").unwrap();
            fs::write(outputs_dir.join("artifact.txt"), "excluded\n").unwrap();
        }
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "claude-code",
        ])
        .assert()
        .success();

    let with_dir = iteration_dir(&cwd).join("eval-edit/with_skill");
    let with = read_json(&with_dir.join("diff-scope.json"));
    assert_eq!(with["files_touched"], 2);
    assert_eq!(with["lines_added"], 2);
    assert_eq!(with["lines_removed"], 1);
    assert_eq!(with["hunks"], 2);
    assert_eq!(
        with["files"],
        json!([
            { "path": "notes.txt", "status": "added", "lines_added": 1, "lines_removed": 0 },
            { "path": "source.txt", "status": "modified", "lines_added": 1, "lines_removed": 1 },
        ])
    );
    assert_eq!(with["patch"]["path"], "diff.patch");
    assert_eq!(with["patch"]["truncated"], false);
    let patch = read_str(&with_dir.join("diff.patch"));
    assert!(patch.contains("-old"), "{patch}");
    assert!(patch.contains("+new"), "{patch}");
    assert!(patch.contains("+one"), "{patch}");
    assert!(
        !patch.contains("artifact.txt"),
        "framework outputs stay out of the patch: {patch}"
    );
    assert_eq!(with["patch"]["bytes"].as_u64().unwrap(), patch.len() as u64);

    let without_dir = iteration_dir(&cwd).join("eval-edit/without_skill");
    let without = read_json(&without_dir.join("diff-scope.json"));
    assert_eq!(without["files_touched"], 0);
    assert_eq!(without["lines_added"], 0);
    assert_eq!(without["lines_removed"], 0);
    assert_eq!(without["hunks"], 0);
    assert_eq!(without["files"], json!([]));
    assert_eq!(
        read_str(&without_dir.join("diff.patch")),
        "",
        "a run that changed nothing still gets a patch, and it is empty"
    );

    // The copy tree Git replaced must not reappear anywhere under the iteration.
    let copied: Vec<_> = walk_paths(&iteration_dir(&cwd))
        .into_iter()
        .filter(|path| path.ends_with("diff-scope-baseline"))
        .collect();
    assert!(
        copied.is_empty(),
        "no baseline is copied any more: {copied:?}"
    );

    skill_eval()
        .current_dir(&cwd)
        .args(["finalize", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "claude-code",
        ])
        .assert()
        .success();
    let benchmark = read_json(&iteration_dir(&cwd).join("benchmark.json"));
    assert_eq!(
        benchmark["diff_scope"],
        json!({
            "with_skill": [{
                "eval_id": "edit",
                "files_touched": 2,
                "lines_added": 2,
                "lines_removed": 1,
                "hunks": 2
            }],
            "without_skill": [{
                "eval_id": "edit",
                "files_touched": 0,
                "lines_added": 0,
                "lines_removed": 0,
                "hunks": 0
            }]
        })
    );
}

#[test]
fn diff_scope_is_captured_before_command_check_setup_and_reused() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "held-out", "prompt": "fix source.txt", "expected_output": "fixed",
          "skill_should_trigger": false, "files": ["source.txt"],
          "assertions": [{ "id": "secret", "type": "command_check",
            "setup_files": ["holdout/secret.txt"], "command": "exit 0" }] } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    fs::write(skill_dir.join("mr-review/evals/source.txt"), "old\n").unwrap();
    fs::create_dir_all(skill_dir.join("mr-review/evals/holdout")).unwrap();
    fs::write(
        skill_dir.join("mr-review/evals/holdout/secret.txt"),
        "injected\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--no-guard"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        let eval_root = Path::new(task["eval_root"].as_str().unwrap());
        let outputs_dir = Path::new(task["outputs_dir"].as_str().unwrap());
        fs::write(eval_root.join("source.txt"), "new\n").unwrap();
        fs::write(outputs_dir.join("final-message.md"), "done").unwrap();
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "claude-code",
        ])
        .assert()
        .success();

    let result_path = iteration_dir(&cwd).join("eval-held-out/with_skill/diff-scope.json");
    let measured = read_json(&result_path);
    assert_eq!(measured["files_touched"], 1);
    assert_eq!(measured["lines_added"], 1);
    assert_eq!(measured["lines_removed"], 1);

    let with_task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["condition"] == "with_skill")
        .unwrap();
    assert!(
        Path::new(with_task["eval_root"].as_str().unwrap())
            .join("holdout/secret.txt")
            .exists(),
        "command-check setup was injected after diff measurement"
    );

    skill_eval()
        .current_dir(&cwd)
        .args(["grade", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "claude-code",
            "--overwrite",
        ])
        .assert()
        .success();
    assert_eq!(read_json(&result_path), measured);
}

#[test]
fn finalize_grades_diff_scope_thresholds_from_the_persisted_measurement() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "focused", "prompt": "fix source.txt", "expected_output": "fixed",
          "skill_should_trigger": false, "files": ["source.txt"],
          "assertions": [{ "id": "small-change", "type": "diff_scope",
            "max_files_touched": 1, "max_lines_changed": 1 }] } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    fs::write(skill_dir.join("mr-review/evals/source.txt"), "old\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--no-guard"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        let eval_root = Path::new(task["eval_root"].as_str().unwrap());
        let outputs_dir = Path::new(task["outputs_dir"].as_str().unwrap());
        fs::write(outputs_dir.join("final-message.md"), "done").unwrap();
        if task["condition"] == "with_skill" {
            fs::write(eval_root.join("source.txt"), "new\n").unwrap();
        }
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "claude-code",
        ])
        .assert()
        .success();
    skill_eval()
        .current_dir(&cwd)
        .args(["finalize", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "claude-code",
        ])
        .assert()
        .success();

    let with = read_json(&iteration_dir(&cwd).join("eval-focused/with_skill/grading.json"));
    assert_eq!(with["summary"]["pass_rate"], 0.0);
    assert_eq!(with["assertion_results"][0]["grader"], "diff_scope");
    assert_eq!(with["assertion_results"][0]["confidence"], 1.0);
    assert!(
        with["assertion_results"][0]["evidence"]
            .as_str()
            .unwrap()
            .contains("lines changed: 2 > 1")
    );

    let without = read_json(&iteration_dir(&cwd).join("eval-focused/without_skill/grading.json"));
    assert_eq!(without["summary"]["pass_rate"], 1.0);
    assert_eq!(without["assertion_results"][0]["passed"], true);
}

#[test]
fn grade_waits_for_a_run_record_before_freezing_diff_scope() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "pending", "prompt": "fix source.txt", "expected_output": "fixed",
          "skill_should_trigger": false, "files": ["source.txt"] } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    fs::write(skill_dir.join("mr-review/evals/source.txt"), "old\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--no-guard"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let with_task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["condition"] == "with_skill")
        .unwrap();
    fs::write(
        Path::new(with_task["eval_root"].as_str().unwrap()).join("source.txt"),
        "new\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["grade", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "claude-code",
        ])
        .assert()
        .success();
    assert!(
        !iteration_dir(&cwd)
            .join("eval-pending/with_skill/diff-scope.json")
            .exists(),
        "an incomplete dispatch must not freeze a partial measurement"
    );
}

#[test]
fn benchmark_diff_scope_is_ordered_by_eval_id_then_run_index() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "zeta", "prompt": "p", "expected_output": "o",
          "skill_should_trigger": false, "runs": 2 },
        { "id": "alpha", "prompt": "p", "expected_output": "o",
          "skill_should_trigger": false, "runs": 2 } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--no-guard"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        fs::write(
            Path::new(task["outputs_dir"].as_str().unwrap()).join("final-message.md"),
            "done",
        )
        .unwrap();
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "claude-code",
        ])
        .assert()
        .success();
    skill_eval()
        .current_dir(&cwd)
        .args(["finalize", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "claude-code",
        ])
        .assert()
        .success();

    let benchmark = read_json(&iteration_dir(&cwd).join("benchmark.json"));
    let order: Vec<_> = benchmark["diff_scope"]["with_skill"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| {
            (
                run["eval_id"].as_str().unwrap().to_string(),
                run["run_index"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        order,
        vec![
            ("alpha".to_string(), 1),
            ("alpha".to_string(), 2),
            ("zeta".to_string(), 1),
            ("zeta".to_string(), 2),
        ]
    );
}

/// Mode B measures the same way Mode A does. Its conditions are two skill
/// revisions rather than skill-versus-none, but the evidence a run produces —
/// metrics and a patch per cell — must not depend on which mode produced it.
#[test]
fn revision_mode_measures_and_captures_the_diff_for_both_arms() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "edit", "prompt": "fix source.txt", "expected_output": "fixed",
          "skill_should_trigger": false, "files": ["source.txt"] } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    fs::write(skill_dir.join("mr-review/evals/source.txt"), "old\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["snapshot", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--label", "baseline"])
        .assert()
        .success();
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "revision", "--no-guard"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        let eval_root = Path::new(task["eval_root"].as_str().unwrap());
        let outputs_dir = Path::new(task["outputs_dir"].as_str().unwrap());
        fs::write(outputs_dir.join("final-message.md"), "done").unwrap();
        fs::write(eval_root.join("source.txt"), "new\n").unwrap();
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "claude-code",
        ])
        .assert()
        .success();

    for condition in ["old_skill", "new_skill"] {
        let cell = iteration_dir(&cwd).join("eval-edit").join(condition);
        let measured = read_json(&cell.join("diff-scope.json"));
        assert_eq!(measured["files_touched"], 1, "{condition}");
        assert_eq!(measured["lines_added"], 1, "{condition}");
        assert_eq!(measured["lines_removed"], 1, "{condition}");
        assert_eq!(measured["files"][0]["path"], "source.txt", "{condition}");
        let patch = read_str(&cell.join("diff.patch"));
        assert!(patch.contains("-old"), "{condition}: {patch}");
        assert!(patch.contains("+new"), "{condition}: {patch}");
        let evidence = read_str(&cell.join("judge-evidence.md"));
        assert!(evidence.contains(&format!("Condition: `{condition}`")));
        assert!(evidence.contains("1 files, +1/-1 lines"));
        assert!(evidence.contains("-old"), "{condition}: {evidence}");
        assert!(evidence.contains("+new"), "{condition}: {evidence}");
    }

    // The codebase and skill provenance #244 requires must survive the change.
    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert!(
        conditions["skill_source"].is_object(),
        "the skill source must still reach conditions.json: {conditions}"
    );
}
