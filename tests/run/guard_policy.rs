//! End-to-end guard-policy resolution and artifact freezing.

use std::fs;

use crate::helpers::*;

#[test]
fn automatic_profiles_compose_and_are_frozen_into_dispatch_and_marker() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{
      "skill_name": "mr-review",
      "evals": [{
        "id": "e1",
        "prompt": "build the app",
        "expected_output": "built",
        "files": ["frontend/package.json", "backend/pyproject.toml"]
      }]
    }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    let eval_dir = skill_dir.join("mr-review/evals");
    fs::create_dir_all(eval_dir.join("frontend")).unwrap();
    fs::write(
        eval_dir.join("frontend/package.json"),
        r#"{"dependencies":{"next":"15.0.0"}}"#,
    )
    .unwrap();
    fs::create_dir_all(eval_dir.join("backend")).unwrap();
    fs::write(eval_dir.join("backend/pyproject.toml"), "").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let policy = &dispatch["tasks"][0]["guard_policy"];
    assert_eq!(
        policy["profiles"],
        serde_json::json!(["framework/nextjs", "language/javascript", "language/python"])
    );
    assert!(
        policy["allow_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command == "npm run dev")
    );
    assert!(
        policy["allow_commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command == "python -m pytest")
    );

    let marker = read_json(
        &cli_env_dir(&cwd, "g1", "with_skill").join(".claude/skills/.slow-powers-eval-guard.json"),
    );
    assert_eq!(marker["guardPolicy"], *policy);
}

#[test]
fn per_eval_guard_replaces_the_default_and_disables_detection() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{
      "skill_name": "mr-review",
      "guard": { "profiles": ["language/rust"] },
      "evals": [{
        "id": "e1",
        "prompt": "serve the app",
        "expected_output": "served",
        "files": ["package.json"],
        "guard": { "allow_commands": ["npm run dev"] }
      }]
    }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    fs::write(
        skill_dir.join("mr-review/evals/package.json"),
        r#"{"dependencies":{"next":"15.0.0"}}"#,
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--dry-run",
            "--no-guard",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let policy = &dispatch["tasks"][0]["guard_policy"];
    assert_eq!(
        policy,
        &serde_json::json!({ "allow_commands": ["npm run dev"] })
    );
}
