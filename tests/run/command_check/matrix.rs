use super::*;

fn timezone_matrix_command() -> &'static str {
    "printf '%s:%s' \"$FIXED\" \"$TZ\"; test \"$TZ\" = UTC"
}

#[test]
fn reports_cells_and_any_failure_fails_finalized_grading() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = json!({
        "skill_name": "mr-review",
        "evals": [{
            "id": "timezone",
            "prompt": "do the task",
            "expected_output": "passes every timezone",
            "skill_should_trigger": false,
            "assertions": [{
                "id": "timezone-matrix",
                "type": "command_check",
                "command": timezone_matrix_command(),
                "env": {
                    "FIXED": "configured"
                },
                "matrix": {
                    "TZ": ["UTC", "Europe/Berlin"]
                },
                "expect_stdout": "^configured:"
            }]
        }]
    });
    let (skill_dir, cwd) = setup(tmp.path(), &serde_json::to_string(&evals).unwrap());
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
    for task in dispatch_tasks(&cwd) {
        let outputs = resolve(&cwd, task["outputs_dir"].as_str().unwrap());
        fs::create_dir_all(&outputs).unwrap();
        fs::write(outputs.join("final-message.md"), "done").unwrap();
    }

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
        .success();

    for condition in ["with_skill", "without_skill"] {
        let run_dir = iteration_dir(&cwd).join("eval-timezone").join(condition);
        let result = read_json(&run_dir.join("command-checks/timezone-matrix.json"));
        assert_eq!(result["passed"], false);
        assert_eq!(result["actual_exit_code"], Value::Null);
        assert_eq!(result["stdout"], "");
        assert_eq!(result["stderr"], "");
        assert_eq!(result["cells"].as_array().unwrap().len(), 2);
        assert_eq!(result["cells"][0]["env"]["FIXED"], "configured");
        assert_eq!(result["cells"][0]["env"]["TZ"], "UTC");
        assert_eq!(result["cells"][0]["passed"], true);
        assert_eq!(result["cells"][1]["env"]["TZ"], "Europe/Berlin");
        assert_eq!(result["cells"][1]["passed"], false);
        assert!(
            result["evidence"]
                .as_str()
                .unwrap()
                .contains("1/2 matrix cells passed")
        );
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["finalize", "--skill-dir"])
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
        .success();

    for condition in ["with_skill", "without_skill"] {
        let grading = read_json(
            &iteration_dir(&cwd)
                .join("eval-timezone")
                .join(condition)
                .join("grading.json"),
        );
        assert_eq!(grading["assertion_results"][0]["passed"], false);
        assert!(
            grading["assertion_results"][0]["evidence"]
                .as_str()
                .unwrap()
                .contains("TZ=Europe/Berlin")
        );
    }
}
