//! Runner-owned command checks across workspace construction, ingest, and finalize.

use crate::helpers::*;
use predicates::str::contains;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const COOL_DESCRIPTOR: &str = r#"label = "cool-custom-harness"

[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "cool-events.jsonl"
parser = "codex-items"

[dispatch]
exec_template = "cool-cli run --cd <eval-root>{model_arg} <dispatch_prompt_path> > <outputs_dir>/cool-events.jsonl"
"#;

fn write_project_descriptor(cwd: &Path) {
    let dir = cwd.join(".eval-magic").join("harnesses");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("cool.toml"), COOL_DESCRIPTOR).unwrap();
}

fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn dispatch_tasks(cwd: &Path) -> Vec<Value> {
    read_json(&iteration_dir(cwd).join("dispatch.json"))["tasks"]
        .as_array()
        .expect("dispatch.json carries tasks[]")
        .clone()
}

fn write_cool_task_result(cwd: &Path, task: &Value) {
    write_task_transcript(
        cwd,
        task,
        "cool-events.jsonl",
        "{\"type\":\"item.completed\",\"item\":{\"id\":\"m1\",\"type\":\"agent_message\",\"text\":\"done\"}}\n",
    );
}

fn setup_exists_command() -> String {
    fixture(&["--require-file", "holdout/secret.txt"])
}

fn held_out_compare_command() -> String {
    fixture(&["--files-equal", "answer.txt", "holdout/expected.txt"])
}

#[test]
fn auto_isolates_and_multi_run_tasks_get_distinct_hidden_envs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = json!({ "skill_name": "mr-review", "evals": [
        { "id": "ordinary-1", "prompt": "p1", "expected_output": "o" },
        { "id": "held-out", "prompt": "p2", "expected_output": "o", "runs": 2,
          "assertions": [{ "id": "secret-test", "type": "command_check",
            "setup_files": ["holdout/secret.txt"], "command": setup_exists_command() }] },
        { "id": "ordinary-2", "prompt": "p3", "expected_output": "o" }
    ] });
    let (skill_dir, cwd) = setup(tmp.path(), &serde_json::to_string(&evals).unwrap());
    fs::create_dir_all(skill_dir.join("mr-review/evals/holdout")).unwrap();
    fs::write(
        skill_dir.join("mr-review/evals/holdout/secret.txt"),
        "secret",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let groups = dispatch["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0]["evals"], json!(["ordinary-1"]));
    assert_eq!(groups[1]["evals"], json!(["held-out"]));
    assert_eq!(groups[2]["evals"], json!(["ordinary-2"]));
    assert_eq!(groups[1]["rationale"], "private codebase");

    let command_tasks: Vec<_> = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|task| task["eval_id"] == "held-out")
        .collect();
    assert_eq!(command_tasks.len(), 4, "2 conditions × 2 runs");
    let roots: std::collections::HashSet<_> = command_tasks
        .iter()
        .map(|task| task["eval_root"].as_str().unwrap())
        .collect();
    assert_eq!(roots.len(), 4, "every command-check task owns its env");
    for task in command_tasks {
        let root = Path::new(task["eval_root"].as_str().unwrap());
        let run_index = task["run_index"].as_u64().unwrap();
        assert!(
            root.to_string_lossy()
                .ends_with(&format!("run-{run_index}")),
            "root carries run suffix: {}",
            root.display()
        );
        assert!(root.exists(), "staged task env exists: {}", root.display());
        assert!(
            !root.join("holdout/secret.txt").exists(),
            "held-out setup is absent before ingest"
        );
        assert_eq!(task["files"], json!([]));
        let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
        assert!(!prompt.contains("holdout/secret.txt"));
    }

    let recorded_roots: std::collections::HashSet<_> = groups[1]["envs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|env| env["dir"].as_str().unwrap())
        .collect();
    assert_eq!(recorded_roots, roots);
}

#[test]
fn missing_setup_source_fails_before_staging() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "held-out", "prompt": "p", "expected_output": "o",
          "assertions": [{ "id": "secret-test", "type": "command_check",
            "setup_files": ["holdout/missing.txt"], "command": "exit 0" }] }
    ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .failure()
        .stderr(contains("command-check setup file not found"))
        .stderr(contains("holdout/missing.txt"));
    assert!(
        !iteration_dir(&cwd).exists(),
        "preflight failure must happen before staging"
    );
}

#[test]
fn root_git_setup_path_is_rejected_before_staging() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "held-out", "prompt": "p", "expected_output": "o",
          "assertions": [{ "id": "secret-test", "type": "command_check",
            "setup_files": [".GIT/config"], "command": "exit 0" }] }
    ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    fs::create_dir_all(skill_dir.join("mr-review/evals/.GIT")).unwrap();
    fs::write(skill_dir.join("mr-review/evals/.GIT/config"), "hostile").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .failure()
        .stderr(contains("reserved runner-owned Git metadata"))
        .stderr(contains(".GIT/config"));
    assert!(
        !iteration_dir(&cwd).exists(),
        "reserved setup path must fail before staging"
    );
}

#[test]
fn runner_ready_descriptor_runs_command_checks() {
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
                "command": setup_exists_command()
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
    for task in &tasks {
        let eval_root = resolve(&cwd, task["eval_root"].as_str().unwrap());
        assert!(!eval_root.join("holdout/secret.txt").exists());
        write_cool_task_result(&cwd, task);
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
        let result = read_json(
            &iteration_dir(&cwd)
                .join("eval-held-out")
                .join(condition)
                .join("command-checks/held-out-check.json"),
        );
        assert_eq!(result["passed"], true);
    }
    assert_eq!(
        read_json(&iteration_dir(&cwd).join("judge-tasks.json"))["total_tasks"],
        0
    );
}

#[test]
fn ingests_without_transcripts_while_guard_is_armed_and_aggregates() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = json!({
        "skill_name": "mr-review",
        "evals": [{
            "id": "held-out",
            "prompt": "write answer.txt",
            "expected_output": "the answer is correct",
            "skill_should_trigger": false,
            "assertions": [{
                "id": "answer-correct",
                "type": "command_check",
                "setup_files": ["holdout/expected.txt"],
                "command": held_out_compare_command()
            }]
        }]
    });
    let (skill_dir, cwd) = setup(tmp.path(), &serde_json::to_string(&evals).unwrap());
    fs::create_dir_all(skill_dir.join("mr-review/evals/holdout")).unwrap();
    fs::write(
        skill_dir.join("mr-review/evals/holdout/expected.txt"),
        "correct",
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
            "--guard",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        let eval_root = Path::new(task["eval_root"].as_str().unwrap());
        assert!(!eval_root.join("holdout/expected.txt").exists());
        assert!(
            eval_root
                .join(".claude/skills/.slow-powers-eval-guard.json")
                .exists(),
            "guard is armed while command checks later run"
        );
        fs::write(
            eval_root.join("answer.txt"),
            if task["condition"] == "with_skill" {
                "correct"
            } else {
                "wrong"
            },
        )
        .unwrap();
        write_default_task_result(&cwd, task, "done");
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

    for condition in ["with_skill", "without_skill"] {
        let result = read_json(
            &iteration_dir(&cwd)
                .join("eval-held-out")
                .join(condition)
                .join("command-checks/answer-correct.json"),
        );
        assert_eq!(result["passed"], condition == "with_skill");
        let task = dispatch["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["condition"] == condition)
            .unwrap();
        assert!(
            Path::new(task["eval_root"].as_str().unwrap())
                .join("holdout/expected.txt")
                .exists(),
            "setup is injected only once ingest grades the task"
        );
    }

    let with_task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["condition"] == "with_skill")
        .unwrap();
    let with_root = Path::new(with_task["eval_root"].as_str().unwrap());
    let with_result_path =
        iteration_dir(&cwd).join("eval-held-out/with_skill/command-checks/answer-correct.json");
    fs::write(with_root.join("answer.txt"), "wrong").unwrap();
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
        .success()
        .stdout(contains("0 executed, 2 reused"));
    assert_eq!(read_json(&with_result_path)["passed"], true);

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
        .success()
        .stdout(contains("2 executed, 0 reused"));
    assert_eq!(read_json(&with_result_path)["passed"], false);

    fs::write(with_root.join("answer.txt"), "correct").unwrap();
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
    assert_eq!(read_json(&with_result_path)["passed"], true);
    assert_eq!(
        read_json(&iteration_dir(&cwd).join("judge-tasks.json"))["total_tasks"],
        0
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
        .success()
        .stdout(contains("Guard still armed"));

    let benchmark = read_json(&iteration_dir(&cwd).join("benchmark.json"));
    assert_eq!(
        benchmark["run_summary"]["with_skill"]["pass_rate"]["mean"],
        1.0
    );
    assert_eq!(
        benchmark["run_summary"]["without_skill"]["pass_rate"]["mean"],
        0.0
    );
}

mod matrix;
mod partial_dispatch;
