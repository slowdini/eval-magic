//! Claude Code hybrid run mode (`--run-mode hybrid`): `claude -p` stream-json
//! dispatch guidance, run-mode persistence + defaulting, the human-followed
//! runbook, and the run-mode combo rejections.

use crate::helpers::*;
use predicates::str::contains;

#[test]
fn claude_hybrid_dispatch_guidance_uses_claude_p() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let assert = skill_eval()
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
            "--run-mode",
            "hybrid",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("claude -p --output-format stream-json"));
    assert!(stdout.contains("--verbose"));
    assert!(stdout.contains("cd <eval-root>"));
    assert!(stdout.contains("claude-events.jsonl"));
    assert!(!stdout.contains("--output-last-message"));

    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(manifest.contains("claude -p --output-format stream-json"));
    assert!(manifest.contains("claude-events.jsonl"));
    assert!(manifest.contains("xargs -0 -P"));

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(conditions["harness"], "claude-code");
    assert_eq!(conditions["run_mode"], "hybrid");
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(dispatch["run_mode"], "hybrid");
}

#[test]
fn claude_hybrid_dispatch_guidance_includes_agent_model_when_provided() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let assert = skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "claude-code",
            "--run-mode",
            "hybrid",
            "--agent-model",
            "opus",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("claude -p --output-format stream-json"));
    assert!(stdout.contains("--model opus"));
}

#[test]
fn claude_defaults_to_interactive_handoff() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "claude-code",
            "--dry-run",
        ])
        .assert()
        .success();

    // No --run-mode → interactive default; no CLI recipe in the manifest.
    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(conditions["run_mode"], "interactive");
    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(!manifest.contains("claude -p"));
}

#[test]
fn claude_hybrid_runbook_is_human_followed_cli_recipe() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "claude-code",
            "--run-mode",
            "hybrid",
            "--dry-run",
        ])
        .assert()
        .success();

    let runbook = read_str(&env_dir(&cwd).join("RUNBOOK.md"));
    assert!(
        runbook.contains("human driving"),
        "hybrid uses the human-followed template: {runbook}"
    );
    assert!(
        runbook.contains("claude -p"),
        "carries the claude -p dispatch recipe: {runbook}"
    );
}

#[test]
fn claude_rejects_run_mode_headless() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "claude-code",
            "--run-mode",
            "headless",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(contains("headless"))
        .stderr(contains("hybrid"));
}

#[test]
fn codex_rejects_run_mode_interactive() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "codex",
            "--run-mode",
            "interactive",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(contains("interactive"))
        .stderr(contains("codex"));
}

#[test]
fn claude_hybrid_rejects_guard() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "claude-code",
            "--run-mode",
            "hybrid",
            "--guard",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(contains("--guard"));
}
