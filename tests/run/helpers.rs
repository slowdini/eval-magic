//! Shared fixtures and helpers for the `run` integration tests.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;

pub const STAGED_MANIFEST: &str = ".slow-powers-eval-manifest.json";
pub const DEFAULT_EVALS: &str = r#"{ "skill_name": "mr-review", "evals": [ { "id": "e1", "prompt": "review this MR", "expected_output": "a review" } ] }"#;

pub fn skill_eval() -> Command {
    Command::cargo_bin("eval-magic").expect("binary `eval-magic` should build")
}

/// Build `<root>/skill-dir/mr-review/{SKILL.md,evals/evals.json}` and a `work`
/// cwd; returns `(skill_dir, cwd)`.
pub fn setup(root: &Path, evals_json: &str) -> (PathBuf, PathBuf) {
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    fs::write(
        skill_sub.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review merge requests\n---\n\nbody\n",
    )
    .unwrap();
    fs::write(skill_sub.join("evals").join("evals.json"), evals_json).unwrap();
    let cwd = root.join("work");
    fs::create_dir_all(&cwd).unwrap();
    (skill_dir, cwd)
}

pub fn iteration_dir(cwd: &Path) -> PathBuf {
    cwd.join(".eval-magic")
        .join("mr-review")
        .join("iteration-1")
}

/// The isolated env dir that becomes the agent-under-test's cwd (in-session
/// dispatch): staging, fixtures, and `RUNBOOK.md` all land under here, below
/// `iteration_dir`.
pub fn env_dir(cwd: &Path) -> PathBuf {
    iteration_dir(cwd).join("env")
}

/// A per-`(group, condition)` Cli env dir — the cwd each `claude -p`/`codex exec`
/// subprocess runs from: `iteration-N/env-<group>-<condition>/`. Each holds only
/// that condition's skill (or none, for the control arm) and its group's fixtures.
pub fn cli_env_dir(cwd: &Path, group: &str, condition: &str) -> PathBuf {
    iteration_dir(cwd).join(format!("env-{group}-{condition}"))
}

/// Staged skill names under the env's harness skills dir (`env/.claude/skills`),
/// excluding the staging manifest, sorted.
pub fn env_staged_entries(cwd: &Path) -> Vec<String> {
    staged_entries(&env_dir(cwd).join(".claude/skills"))
}

pub fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

pub fn read_str(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// Names directly under `.claude/skills` (or `.agents/skills`), excluding the
/// staging manifest, sorted.
pub fn staged_entries(skills_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(skills_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != STAGED_MANIFEST)
        .collect();
    names.sort();
    names
}
