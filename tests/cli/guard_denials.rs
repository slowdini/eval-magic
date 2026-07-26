//! Guard-denial collection and benchmark validity warnings.

use crate::helpers::{canonical_root, skill_eval};
use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn setup_aggregate(root: &Path) -> (PathBuf, String, PathBuf, PathBuf) {
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    fs::create_dir_all(&skill_sub).unwrap();
    fs::write(
        skill_sub.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
    )
    .unwrap();
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();
    let cwd = root.join("work");
    let iteration_dir = cwd
        .join(".eval-magic")
        .join("mr-review")
        .join("iteration-1");
    fs::create_dir_all(&iteration_dir).unwrap();
    (skill_dir, skill_md, iteration_dir, cwd)
}

fn write_grading(iteration_dir: &Path, condition: &str) {
    let run_dir = iteration_dir.join("eval-e1").join(condition);
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(
        run_dir.join("grading.json"),
        serde_json::to_string(&serde_json::json!({
            "assertion_results": [],
            "summary": {"passed": 1, "failed": 0, "total": 1, "pass_rate": 1.0},
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        run_dir.join("timing.json"),
        r#"{"total_tokens":100,"duration_ms":1}"#,
    )
    .unwrap();
}

fn aggregate_command(cwd: &Path, skill_dir: &Path) -> Command {
    let mut command = skill_eval();
    command
        .current_dir(cwd)
        .arg("aggregate")
        .arg("--skill-dir")
        .arg(skill_dir)
        .arg("--skill")
        .arg("mr-review")
        .arg("--iteration")
        .arg("1");
    command
}

fn write_conditions(iteration_dir: &Path, skill_md: &str) {
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&serde_json::json!({
            "mode": "new-skill",
            "conditions": [
                {"name": "with_skill", "skill_path": skill_md},
                {"name": "without_skill", "skill_path": null},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
}

fn setup_guard_denial_iteration(tmp: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
    use serde_json::json;

    let root = fs::canonicalize(tmp.path()).unwrap();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    fs::create_dir_all(&skill_sub).unwrap();
    fs::write(
        skill_sub.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
    )
    .unwrap();
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join(".eval-magic")
        .join("mr-review")
        .join("iteration-1");
    fs::create_dir_all(&iteration_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "revision",
            "conditions": [
                {"name": "old_skill", "skill_path": skill_md},
                {"name": "new_skill", "skill_path": skill_md},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "codex",
        }))
        .unwrap(),
    )
    .unwrap();
    (skill_dir, cwd, iteration_dir)
}

fn raw_denial(tool: &str, reason: &str, targets: &[&str]) -> String {
    serde_json::json!({
        "timestamp": "2026-07-26T12:00:00.000Z",
        "harness": "codex",
        "tool": tool,
        "reason": reason,
        "resolved_targets": targets,
        "input_keys": ["command"],
    })
    .to_string()
}

fn write_raw_denials(eval_root: &Path, lines: &[String]) -> PathBuf {
    let path = eval_root
        .join(".eval-magic-outputs")
        .join("guard-denials.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
    path
}

#[test]
fn detect_stray_writes_collects_guard_denials_without_run_records() {
    use serde_json::json;

    let tmp = TempDir::new().unwrap();
    let (skill_dir, cwd, iteration_dir) = setup_guard_denial_iteration(&tmp);
    let single_root = iteration_dir.join("env-g1-old_skill");
    let repeated_root = iteration_dir.join("env-g2-new_skill-run-2");
    write_raw_denials(
        &single_root,
        &[
            raw_denial("Bash", "outside redirect", &["/etc/out"]),
            raw_denial("apply_patch", "outside patch", &["/etc/source"]),
        ],
    );
    write_raw_denials(
        &repeated_root,
        &[raw_denial("Write", "outside write", &["/etc/passwd"])],
    );
    fs::write(
        iteration_dir.join("dispatch.json"),
        serde_json::to_string(&json!({
            "tasks": [
                {
                    "eval_id": "e2",
                    "condition": "new_skill",
                    "run_index": 2,
                    "eval_root": repeated_root,
                },
                {
                    "eval_id": "e1",
                    "condition": "old_skill",
                    "eval_root": single_root,
                },
            ],
        }))
        .unwrap(),
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .arg("detect-stray-writes")
        .arg("--skill-dir")
        .arg(&skill_dir)
        .arg("--skill")
        .arg("mr-review")
        .arg("--iteration")
        .arg("1")
        .assert()
        .success()
        .stdout(contains("guard-denials.json"))
        .stderr(contains("3 guard denial"));

    let report: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(iteration_dir.join("guard-denials.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["iteration"], 1);
    assert_eq!(report["total_denials"], 3);
    assert_eq!(report["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(report["tasks"][0]["eval_id"], "e1");
    assert_eq!(report["tasks"][0]["condition"], "old_skill");
    assert!(report["tasks"][0].get("run_index").is_none());
    assert_eq!(report["tasks"][0]["denial_count"], 2);
    assert_eq!(report["tasks"][1]["eval_id"], "e2");
    assert_eq!(report["tasks"][1]["condition"], "new_skill");
    assert_eq!(report["tasks"][1]["run_index"], 2);
    assert_eq!(report["tasks"][1]["denial_count"], 1);
}

#[test]
fn detect_stray_writes_fails_on_malformed_guard_denial_with_path_and_line() {
    use serde_json::json;

    let tmp = TempDir::new().unwrap();
    let (skill_dir, cwd, iteration_dir) = setup_guard_denial_iteration(&tmp);
    let eval_root = iteration_dir.join("env-g1-old_skill");
    let log_path = write_raw_denials(
        &eval_root,
        &[
            raw_denial("Bash", "outside redirect", &["/etc/out"]),
            "not json".to_string(),
        ],
    );
    fs::write(
        iteration_dir.join("dispatch.json"),
        serde_json::to_string(&json!({
            "tasks": [{
                "eval_id": "e1",
                "condition": "old_skill",
                "eval_root": eval_root,
            }],
        }))
        .unwrap(),
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .arg("detect-stray-writes")
        .arg("--skill-dir")
        .arg(&skill_dir)
        .arg("--skill")
        .arg("mr-review")
        .arg("--iteration")
        .arg("1")
        .assert()
        .failure()
        .stderr(contains(format!("{}:2", log_path.display())));
}

/// Every affected task gets one validity warning, including legitimate
/// boundary blocks because each denial changed behavior.
#[test]
fn aggregate_surfaces_one_guard_denial_warning_per_affected_task() {
    use serde_json::json;

    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_aggregate(&root);
    write_conditions(&iteration_dir, &skill_md);
    for cond in ["with_skill", "without_skill"] {
        write_grading(&iteration_dir, cond);
    }
    fs::write(
        iteration_dir.join("guard-denials.json"),
        serde_json::to_string(&json!({
            "generated": "2026-07-26T12:00:00.000Z",
            "iteration": 1,
            "total_denials": 3,
            "tasks": [
                {
                    "eval_id": "e1",
                    "condition": "with_skill",
                    "denial_count": 2,
                    "denials": [
                        {
                            "timestamp": "2026-07-26T11:00:00.000Z",
                            "harness": "codex",
                            "tool": "Bash",
                            "reason": "outside",
                            "resolved_targets": ["/etc/out"],
                            "input_keys": ["command"]
                        },
                        {
                            "timestamp": "2026-07-26T11:01:00.000Z",
                            "harness": "codex",
                            "tool": "apply_patch",
                            "reason": "unknown target",
                            "resolved_targets": [],
                            "input_keys": ["command"]
                        }
                    ]
                },
                {
                    "eval_id": "e1",
                    "condition": "without_skill",
                    "run_index": 2,
                    "denial_count": 1,
                    "denials": [{
                        "timestamp": "2026-07-26T11:02:00.000Z",
                        "harness": "codex",
                        "tool": "Write",
                        "reason": "outside",
                        "resolved_targets": ["/etc/passwd"],
                        "input_keys": ["file_path"]
                    }]
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    aggregate_command(&cwd, &skill_dir).assert().success();

    let benchmark: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("benchmark.json")).unwrap())
            .unwrap();
    let warnings: Vec<&str> = benchmark["validity_warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|warning| warning.as_str())
        .filter(|warning| warning.contains("guard denial"))
        .collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings.iter().any(|warning| {
        warning.contains("e1/with_skill")
            && warning.contains("2 guard denial")
            && warning.contains("guard-denials.json")
    }));
    assert!(warnings.iter().any(|warning| {
        warning.contains("e1/without_skill/run-2")
            && warning.contains("1 guard denial")
            && warning.contains("guard-denials.json")
    }));
}
