//! Isolation-group batching during `run`: how the setup phase groups evals into
//! environments and records the plan in `dispatch.json`. Covers the per-(group,
//! condition) env split that closes the condition-isolation gap — emitted for every
//! run now, including the bare default invocation — and the explicit
//! `isolation: isolated` hint that fans a second group out into its own envs.

use crate::helpers::*;
use serde_json::json;
use std::fs;

const TWO_EVALS_ONE_ISOLATED: &str = r#"{ "skill_name": "mr-review", "evals": [
    { "id": "e1", "prompt": "p1", "expected_output": "o", "files": ["a.txt"] },
    { "id": "e2", "prompt": "p2", "expected_output": "o", "files": ["b.txt"], "isolation": "isolated" } ] }"#;

fn write_fixtures(skill_dir: &std::path::Path) {
    fs::write(skill_dir.join("mr-review/evals/a.txt"), "AAA").unwrap();
    fs::write(skill_dir.join("mr-review/evals/b.txt"), "BBB").unwrap();
}

#[test]
fn single_group_emits_groups_key_and_per_condition_envs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    // Even the bare default invocation now splits the env per (group, condition) and
    // always records a `groups` summary — the single-env, no-groups shape is gone.
    assert!(cli_env_dir(&cwd, "g1", "with_skill").exists());
    assert!(cli_env_dir(&cwd, "g1", "without_skill").exists());
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let groups = dispatch["groups"]
        .as_array()
        .expect("groups summary present even for a single group");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["id"], "g1");

    // A single group means no per-task group tag, but each task still carries the
    // per-condition env it runs in via eval_root.
    for task in dispatch["tasks"].as_array().unwrap() {
        assert!(task.get("group").is_none(), "single group: no tag: {task}");
        let cond = task["condition"].as_str().unwrap();
        let eval_root = task["eval_root"].as_str().expect("task carries eval_root");
        assert!(
            eval_root.ends_with(&format!("env-g1-{cond}")),
            "eval_root points at the per-condition env: {eval_root}"
        );
    }
}

#[test]
fn cli_single_group_emits_groups_and_splits_env_per_condition() {
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
            "--harness",
            "claude-code",
            "--dry-run",
        ])
        .assert()
        .success();

    // Even a single group splits the Cli env per condition: the with_skill env holds
    // the skill, the control arm's env holds none — physical condition isolation.
    let with_env = cli_env_dir(&cwd, "g1", "with_skill");
    let without_env = cli_env_dir(&cwd, "g1", "without_skill");
    assert!(
        with_env
            .join(".claude/skills/slow-powers-eval-1-with_skill__mr-review")
            .exists()
    );
    assert!(
        !without_env
            .join(".claude/skills/slow-powers-eval-1-with_skill__mr-review")
            .exists(),
        "the control arm's env contains no staged skill"
    );

    // The plan is recorded for the executing human: one group, its env per condition.
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let groups = dispatch["groups"]
        .as_array()
        .expect("groups summary present");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["id"], "g1");
    assert_eq!(groups[0]["evals"], json!(["e1"]));

    // Each task carries the env it runs in (the recipe `cd`s into it). A single
    // group means no group tag.
    for task in dispatch["tasks"].as_array().unwrap() {
        assert!(task.get("group").is_none(), "single group: no tag: {task}");
        let eval_root = task["eval_root"]
            .as_str()
            .expect("Cli task carries eval_root");
        let cond = task["condition"].as_str().unwrap();
        assert!(
            eval_root.ends_with(&format!("env-g1-{cond}")),
            "eval_root points at the per-condition env: {eval_root}"
        );
    }
}

#[test]
fn isolated_hint_splits_into_two_groups() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), TWO_EVALS_ONE_ISOLATED);
    write_fixtures(&skill_dir);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let groups = dispatch["groups"]
        .as_array()
        .expect("groups summary present");
    assert_eq!(groups.len(), 2, "the isolated eval forms its own group");
    assert_eq!(groups[0]["evals"], json!(["e1"]));
    assert_eq!(groups[1]["evals"], json!(["e2"]));
    assert!(
        groups[1]["rationale"]
            .as_str()
            .unwrap()
            .contains("isolated"),
        "second group's rationale names the hint: {}",
        groups[1]["rationale"]
    );

    // With two groups, tasks are tagged with their group.
    let e2_task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["eval_id"] == "e2" && t["condition"] == "with_skill")
        .unwrap();
    assert_eq!(e2_task["group"], "g2");

    // Each group gets its own per-condition envs, holding only that group's fixtures —
    // g1's a.txt never leaks into g2's env and vice versa.
    assert_eq!(
        read_str(&cli_env_dir(&cwd, "g1", "with_skill").join("a.txt")),
        "AAA"
    );
    assert!(
        !cli_env_dir(&cwd, "g1", "with_skill").join("b.txt").exists(),
        "the isolated group's fixture is not staged into g1's env"
    );
    assert_eq!(
        read_str(&cli_env_dir(&cwd, "g2", "with_skill").join("b.txt")),
        "BBB"
    );
    assert!(
        !cli_env_dir(&cwd, "g2", "with_skill").join("a.txt").exists(),
        "g1's fixture is not staged into the isolated group's env"
    );
}

#[test]
fn isolated_hint_splits_into_separate_envs_cli() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), TWO_EVALS_ONE_ISOLATED);
    write_fixtures(&skill_dir);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "codex", "--dry-run"])
        .assert()
        .success();

    // Each group gets its own per-condition envs, holding only that group's fixtures.
    assert_eq!(
        read_str(&cli_env_dir(&cwd, "g1", "with_skill").join("a.txt")),
        "AAA"
    );
    assert!(!cli_env_dir(&cwd, "g1", "with_skill").join("b.txt").exists());
    assert_eq!(
        read_str(&cli_env_dir(&cwd, "g2", "with_skill").join("b.txt")),
        "BBB"
    );
    assert!(!cli_env_dir(&cwd, "g2", "with_skill").join("a.txt").exists());

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(dispatch["groups"].as_array().unwrap().len(), 2);
    let e2_task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["eval_id"] == "e2" && t["condition"] == "with_skill")
        .unwrap();
    assert_eq!(e2_task["group"], "g2");
    assert!(
        e2_task["eval_root"]
            .as_str()
            .unwrap()
            .ends_with("env-g2-with_skill")
    );
}
