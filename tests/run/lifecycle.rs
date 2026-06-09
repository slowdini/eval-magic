//! Guard install/teardown, workspace reclamation, run-nonce namespacing,
//! bootstrap framing, and `--only` filtering.

use crate::helpers::*;
use predicates::str::contains;
use std::fs;
use std::path::Path;

#[test]
fn guard_installs_pretooluse_hook_and_teardown_guard_removes_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let settings = cwd.join(".claude/settings.local.json");

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();
    assert!(settings.exists());
    let parsed = read_json(&settings);
    assert!(
        parsed["hooks"]["PreToolUse"][0]["matcher"]
            .as_str()
            .unwrap()
            .contains("Write")
    );

    skill_eval()
        .current_dir(&cwd)
        .args(["teardown-guard", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success();
    assert!(!settings.exists());
}

#[test]
fn teardown_removes_guard_and_staged_skill_set() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let settings = cwd.join(".claude/settings.local.json");
    let staged = cwd.join(".claude/skills");

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();
    assert!(settings.exists());
    assert!(staged.exists());

    skill_eval()
        .current_dir(&cwd)
        .args(["teardown", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success();
    assert!(!settings.exists());
    assert!(!staged.exists());
    assert!(!cwd.join(".claude").exists());
    assert!(!cwd.join("skills-workspace").exists());
}

#[test]
fn teardown_preserves_iteration_with_uncommitted_results() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill"])
        .assert()
        .success();

    // Simulate a graded-but-not-promoted run.
    fs::write(
        iteration_dir(&cwd).join("benchmark.json"),
        "{\"delta\":{\"pass_rate\":0.4}}\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["teardown", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success()
        .stderr(contains("iteration-1"))
        .stderr(contains("promote-baseline"));

    assert!(iteration_dir(&cwd).exists());
}

#[test]
fn normal_run_does_not_install_guard() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();
    assert!(!cwd.join(".claude/settings.local.json").exists());
}

#[test]
fn namespaces_agent_description_and_records_run_nonce() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let nonce = dispatch["run_nonce"].as_str().unwrap();
    assert!(!nonce.is_empty());
    for task in dispatch["tasks"].as_array().unwrap() {
        let condition = task["condition"].as_str().unwrap();
        let desc = task["agent_description"].as_str().unwrap();
        assert!(desc.ends_with(&format!(":{condition}:i1-{nonce}")));
    }
    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(conditions["run_nonce"].as_str().unwrap(), nonce);
}

#[test]
fn bootstrap_content_prepended_before_available_skills() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let bootstrap = cwd.join("my-bootstrap.md");
    fs::write(&bootstrap, "MY CUSTOM EVAL FRAMING").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--bootstrap"])
        .arg(&bootstrap)
        .arg("--dry-run")
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["condition"] == "with_skill")
        .unwrap();
    let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
    let boot_idx = prompt.find("MY CUSTOM EVAL FRAMING").unwrap();
    let list_idx = prompt
        .find("The following skills are available for use with the Skill tool:")
        .unwrap();
    assert!(list_idx > boot_idx);
}

#[test]
fn only_restricts_dispatches_to_named_ids() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review MR 1", "expected_output": "a review" },
        { "id": "e2", "prompt": "review MR 2", "expected_output": "a review" } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--only",
            "e1",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(contains("1 evals × 2 conditions"));

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let ids: Vec<&str> = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["eval_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["e1", "e1"]);
}

#[test]
fn only_with_unknown_id_exits_nonzero() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--only",
            "nope",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(contains("unknown eval id(s): nope"));
}
