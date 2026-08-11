//! Cross-harness eval-agent environment configuration and provenance.

use std::fs;

use predicates::prelude::PredicateBooleanExt;

use crate::helpers::{DEFAULT_EVALS, iteration_dir, read_json, read_str, setup, skill_eval};

#[test]
fn descriptor_defaults_and_cli_overrides_are_recorded_and_rendered() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let descriptor_dir = cwd.join(".eval-magic").join("harnesses");
    fs::create_dir_all(&descriptor_dir).unwrap();
    fs::write(
        descriptor_dir.join("claude-code.toml"),
        "label = \"claude-code\"\n\n[dispatch.env]\nTZ = \"UTC\"\nMODE = \"descriptor\"\n",
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
            "--harness",
            "claude-code",
            "--dry-run",
            "--agent-env",
            "MODE=cli",
            "--agent-env",
            "EMPTY=",
        ])
        .assert()
        .success();

    let iteration = iteration_dir(&cwd);
    let expected = serde_json::json!({
        "EMPTY": "",
        "MODE": "cli",
        "TZ": "UTC"
    });
    assert_eq!(
        read_json(&iteration.join("conditions.json"))["agent_env"],
        expected
    );
    assert_eq!(
        read_json(&iteration.join("dispatch.json"))["agent_env"],
        expected
    );

    let manifest = read_str(&iteration.join("dispatch-manifest.md"));
    assert!(
        manifest.contains("export EMPTY=\nexport MODE=cli\nexport TZ=UTC"),
        "{manifest}"
    );
    let runbook = read_str(&iteration.join("RUNBOOK.md"));
    let judge = runbook
        .split_once("## 2. Dispatch the judge agents, then finalize")
        .unwrap()
        .1;
    assert!(!judge.contains("export TZ=UTC"), "{judge}");
}

#[test]
fn cli_agent_environment_renders_for_every_builtin_harness() {
    let tmp = tempfile::TempDir::new().unwrap();
    for harness in ["claude-code", "cline", "codex", "opencode"] {
        let root = tmp.path().join(harness);
        let (skill_dir, cwd) = setup(&root, DEFAULT_EVALS);

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
                harness,
                "--dry-run",
                "--agent-env",
                "TZ=UTC",
            ])
            .assert()
            .success();

        let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
        assert!(manifest.contains("export TZ=UTC"), "{harness}: {manifest}");
    }
}

#[test]
fn invalid_agent_environment_fails_before_creating_a_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--agent-env", "GIT_DIR=/outside"])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("GIT_DIR").and(predicates::str::contains("task repository")),
        );

    assert!(!cwd.join(".eval-magic").join("mr-review").exists());
}

#[test]
fn dispatch_task_revalidates_persisted_agent_environment() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{
      "skill_name": "mr-review",
      "evals": [{
        "id": "clarify",
        "prompt": "Fix the date.",
        "expected_output": "asks before editing",
        "turns": [{ "prompt": "It is date-only.", "deliver_when": "always" }]
      }]
    }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch_path = iteration_dir(&cwd).join("dispatch.json");
    let mut dispatch = read_json(&dispatch_path);
    dispatch["agent_env"] = serde_json::json!({ "BAD-NAME": "value" });
    fs::write(
        &dispatch_path,
        format!("{}\n", serde_json::to_string_pretty(&dispatch).unwrap()),
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["dispatch-task", "--dispatch"])
        .arg(&dispatch_path)
        .args(["--task-index", "0"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("BAD-NAME"));
}
