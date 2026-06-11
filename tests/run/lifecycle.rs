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
fn runs_flag_expands_dispatches_into_run_dirs() {
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
            "--runs",
            "2",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(contains(
            "8 dispatches required (2 evals × 2 conditions × 2 runs)",
        ));

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(dispatch["runs"], serde_json::json!(2));
    let tasks = dispatch["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 8);

    let mut descriptions = std::collections::HashSet::new();
    for task in tasks {
        let k = task["run_index"].as_u64().unwrap();
        assert!(k == 1 || k == 2);
        let run_seg = format!("/run-{k}/");
        assert!(
            task["run_record_path"].as_str().unwrap().contains(&run_seg),
            "run.json not under its run dir: {}",
            task["run_record_path"]
        );
        assert!(task["outputs_dir"].as_str().unwrap().contains(&run_seg));
        let desc = task["agent_description"].as_str().unwrap();
        assert!(
            desc.contains(&format!(":r{k}:")),
            "missing run segment in description: {desc}"
        );
        assert!(descriptions.insert(desc.to_string()), "duplicate: {desc}");
    }
    for eval in ["e1", "e2"] {
        for cond in ["with_skill", "without_skill"] {
            for k in [1, 2] {
                let run_dir = iteration_dir(&cwd)
                    .join(format!("eval-{eval}"))
                    .join(cond)
                    .join(format!("run-{k}"));
                assert!(run_dir.join("outputs").is_dir(), "missing {run_dir:?}");
            }
        }
    }
}

#[test]
fn runs_one_keeps_flat_single_run_layout() {
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
            "--runs",
            "1",
            "--dry-run",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        assert!(task.get("run_index").is_none(), "run_index on single run");
        assert!(!task["run_record_path"].as_str().unwrap().contains("/run-"));
    }
    let cond_dir = iteration_dir(&cwd).join("eval-e1").join("with_skill");
    assert!(cond_dir.join("outputs").is_dir());
    assert!(!cond_dir.join("run-1").exists());
}

#[test]
fn runs_zero_is_rejected() {
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
            "--runs",
            "0",
            "--dry-run",
        ])
        .assert()
        .failure();
}

#[test]
fn per_eval_runs_overrides_the_flag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review MR 1", "expected_output": "a review", "runs": 3 },
        { "id": "e2", "prompt": "review MR 2", "expected_output": "a review" } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let tasks = dispatch["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 8, "3 runs × 2 conds for e1 + 1 run × 2 for e2");
    let e1_indices: Vec<u64> = tasks
        .iter()
        .filter(|t| t["eval_id"] == "e1" && t["condition"] == "with_skill")
        .map(|t| t["run_index"].as_u64().unwrap())
        .collect();
    assert_eq!(e1_indices, vec![1, 2, 3]);
    for task in tasks.iter().filter(|t| t["eval_id"] == "e2") {
        assert!(task.get("run_index").is_none());
        assert!(!task["run_record_path"].as_str().unwrap().contains("/run-"));
    }
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
