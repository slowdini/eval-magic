use super::*;

fn append_after_setup_command() -> String {
    fixture(&[
        "--require-file",
        "holdout/secret.txt",
        "--text",
        "x",
        "--append",
        "command-runs.txt",
    ])
}

fn tree_snapshot(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut snapshot = walk_paths(root)
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let contents = path.is_file().then(|| fs::read(&path).unwrap());
            (relative, contents)
        })
        .collect::<Vec<_>>();
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
}

#[test]
fn grades_only_completed_tasks_then_resumes_idempotently() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = json!({
        "skill_name": "mr-review",
        "evals": [{
            "id": "held-out",
            "prompt": "do the task",
            "expected_output": "passes held-out check",
            "skill_should_trigger": false,
            "assertions": [{
                "id": "held-out-check",
                "type": "command_check",
                "setup_files": ["holdout/secret.txt"],
                "command": append_after_setup_command()
            }]
        }]
    });
    let (skill_dir, cwd) = setup(tmp.path(), &serde_json::to_string(&evals).unwrap());
    fs::create_dir_all(skill_dir.join("mr-review/evals/holdout")).unwrap();
    fs::write(
        skill_dir.join("mr-review/evals/holdout/secret.txt"),
        "secret",
    )
    .unwrap();
    write_project_descriptor(&cwd);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--harness",
            "cool-custom-harness",
        ])
        .assert()
        .success();

    let tasks = dispatch_tasks(&cwd);
    assert_eq!(tasks.len(), 2);
    let completed = &tasks[0];
    let incomplete = &tasks[1];
    write_cool_task_result(&cwd, completed);

    let completed_root = resolve(&cwd, completed["eval_root"].as_str().unwrap());
    let incomplete_root = resolve(&cwd, incomplete["eval_root"].as_str().unwrap());
    let completed_result = resolve(&cwd, completed["run_record_path"].as_str().unwrap())
        .parent()
        .unwrap()
        .join("command-checks/held-out-check.json");
    let incomplete_result = resolve(&cwd, incomplete["run_record_path"].as_str().unwrap())
        .parent()
        .unwrap()
        .join("command-checks/held-out-check.json");
    let incomplete_before = tree_snapshot(&incomplete_root);

    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "cool-custom-harness",
            "--iteration",
            "1",
        ])
        .assert()
        .success()
        .stdout(contains(
            "Command checks: 1 executed, 0 reused, 0 failed, 1 skipped (missing run.json)",
        ));

    assert!(completed_result.exists());
    assert_eq!(
        fs::read_to_string(completed_root.join("command-runs.txt")).unwrap(),
        "x"
    );
    assert!(!incomplete_result.exists());
    assert!(!incomplete_root.join("holdout/secret.txt").exists());
    assert!(!incomplete_root.join("command-runs.txt").exists());
    assert_eq!(tree_snapshot(&incomplete_root), incomplete_before);

    write_cool_task_result(&cwd, incomplete);
    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "cool-custom-harness",
            "--iteration",
            "1",
        ])
        .assert()
        .success()
        .stdout(contains(
            "Command checks: 1 executed, 1 reused, 0 failed, 0 skipped (missing run.json)",
        ));

    assert_eq!(
        fs::read_to_string(completed_root.join("command-runs.txt")).unwrap(),
        "x"
    );
    assert_eq!(
        fs::read_to_string(incomplete_root.join("command-runs.txt")).unwrap(),
        "x"
    );
    assert!(incomplete_result.exists());

    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "cool-custom-harness",
            "--iteration",
            "1",
        ])
        .assert()
        .success()
        .stdout(contains(
            "Command checks: 0 executed, 2 reused, 0 failed, 0 skipped (missing run.json)",
        ));

    assert_eq!(
        fs::read_to_string(completed_root.join("command-runs.txt")).unwrap(),
        "x"
    );
    assert_eq!(
        fs::read_to_string(incomplete_root.join("command-runs.txt")).unwrap(),
        "x"
    );
}
