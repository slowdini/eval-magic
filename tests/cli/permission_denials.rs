//! Permission-denial validity warnings in `benchmark.json`.
//!
//! A refused tool call leaves the dispatch exiting 0 and the run grading
//! normally, so the warning is the only signal short of reading transcripts.

use crate::helpers::{canonical_root, skill_eval};
use assert_cmd::Command;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

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

fn write_conditions(iteration_dir: &Path, skill_md: &str) {
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [
                {"name": "with_skill", "skill_path": skill_md},
                {"name": "without_skill", "skill_path": null},
            ],
            "timestamp": "2026-07-29T12:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
}

fn write_grading(iteration_dir: &Path, condition: &str, run_index: Option<u32>) {
    let mut run_dir = iteration_dir.join("eval-e1").join(condition);
    if let Some(index) = run_index {
        run_dir = run_dir.join(format!("run-{index}"));
    }
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(
        run_dir.join("grading.json"),
        serde_json::to_string(&json!({
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

fn validity_warnings(iteration_dir: &Path) -> Vec<String> {
    let benchmark: Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("benchmark.json")).unwrap())
            .unwrap();
    benchmark["validity_warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|warning| warning.as_str().map(str::to_string))
        .collect()
}

/// A harness refusal denial record, as record-runs writes it.
fn denial(tool: &str, reason: &str, guard_attributed: bool) -> Value {
    json!({
        "tool": tool,
        "reason": reason,
        "input_keys": ["command"],
        "guard_attributed": guard_attributed,
    })
}

#[test]
fn aggregate_surfaces_one_permission_denial_warning_per_affected_task() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_aggregate(&root);
    write_conditions(&iteration_dir, &skill_md);
    write_grading(&iteration_dir, "with_skill", None);
    write_grading(&iteration_dir, "without_skill", None);
    fs::write(
        iteration_dir.join("permission-denials.json"),
        serde_json::to_string(&json!({
            "generated": "2026-07-29T12:00:00.000Z",
            "iteration": 1,
            "total_denials": 3,
            "tasks": [
                {
                    "eval_id": "e1",
                    "condition": "with_skill",
                    "denial_count": 2,
                    "guard_attributed_count": 0,
                    "denials": [
                        denial("Bash", "This command requires approval", false),
                        denial("Bash", "This command requires approval", false),
                    ],
                },
                {
                    "eval_id": "e1",
                    "condition": "without_skill",
                    "run_index": 2,
                    "denial_count": 1,
                    "guard_attributed_count": 0,
                    "denials": [denial("Write", "This tool requires approval", false)],
                },
            ],
        }))
        .unwrap(),
    )
    .unwrap();

    aggregate_command(&cwd, &skill_dir).assert().success();

    let warnings: Vec<String> = validity_warnings(&iteration_dir)
        .into_iter()
        .filter(|warning| warning.contains("permission-denied"))
        .collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings.iter().any(|warning| {
            warning.contains("e1/with_skill")
                && warning.contains("2 permission-denied tool calls")
                && warning.contains("permission-denials.json")
        }),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|warning| {
            warning.contains("e1/without_skill/run-2")
                && warning.contains("1 permission-denied tool call")
                && warning.contains("permission-denials.json")
        }),
        "{warnings:?}"
    );
}

#[test]
fn aggregate_leaves_guard_attributed_denials_to_the_guard_warning() {
    // The guard denies through the same permission mechanism, so its blocks show
    // up in both reports. Warning from both would double-count one denial and
    // bury the refusals only this report can see.
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_aggregate(&root);
    write_conditions(&iteration_dir, &skill_md);
    write_grading(&iteration_dir, "with_skill", None);
    write_grading(&iteration_dir, "without_skill", None);
    fs::write(
        iteration_dir.join("permission-denials.json"),
        serde_json::to_string(&json!({
            "generated": "2026-07-29T12:00:00.000Z",
            "iteration": 1,
            "total_denials": 1,
            "tasks": [{
                "eval_id": "e1",
                "condition": "with_skill",
                "denial_count": 1,
                "guard_attributed_count": 1,
                "denials": [denial(
                    "Write",
                    "eval guard: Write to /etc/passwd is outside the eval sandbox",
                    true,
                )],
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        iteration_dir.join("guard-denials.json"),
        serde_json::to_string(&json!({
            "generated": "2026-07-29T12:00:00.000Z",
            "iteration": 1,
            "total_denials": 1,
            "tasks": [{
                "eval_id": "e1",
                "condition": "with_skill",
                "denial_count": 1,
                "denials": [{
                    "timestamp": "2026-07-29T11:00:00.000Z",
                    "harness": "claude-code",
                    "tool": "Write",
                    "reason": "eval guard: Write to /etc/passwd is outside the eval sandbox",
                    "resolved_targets": ["/etc/passwd"],
                    "input_keys": ["file_path"],
                }],
            }],
        }))
        .unwrap(),
    )
    .unwrap();

    aggregate_command(&cwd, &skill_dir).assert().success();

    let warnings = validity_warnings(&iteration_dir);
    let about_this_task: Vec<&String> = warnings
        .iter()
        .filter(|warning| warning.contains("e1/with_skill"))
        .collect();
    assert_eq!(about_this_task.len(), 1, "{about_this_task:?}");
    assert!(about_this_task[0].contains("guard denial"), "{warnings:?}");
    assert!(
        !warnings
            .iter()
            .any(|warning| warning.contains("permission-denied")),
        "{warnings:?}"
    );
}
