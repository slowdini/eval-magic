//! Claude Code CLI run modes (`--run-mode hybrid` / `headless`): `claude -p`
//! stream-json dispatch guidance, run-mode persistence + defaulting, the
//! human-followed runbook, the write guard under Cli dispatch, and the remaining
//! run-mode combo rejections (Codex interactive).

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
fn claude_headless_records_mode_and_human_runbook() {
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
        .success();

    // Headless rides the same Cli mechanism as hybrid; the run mode is persisted
    // distinctly so every post-dispatch command can carry it.
    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(conditions["run_mode"], "headless");
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(dispatch["run_mode"], "headless");
    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(manifest.contains("claude -p --output-format stream-json"));

    // The runbook is the shared human-followed template carrying the claude -p
    // recipe and headless-threaded pipeline commands.
    let runbook = read_str(&env_dir(&cwd).join("RUNBOOK.md"));
    assert!(
        runbook.contains("human driving"),
        "headless uses the human-followed template: {runbook}"
    );
    assert!(
        runbook.contains("claude -p"),
        "carries the claude -p dispatch recipe: {runbook}"
    );
    assert!(
        runbook.contains("--harness claude-code"),
        "pipeline commands carry --harness claude-code: {runbook}"
    );
    assert!(
        runbook.contains("--run-mode headless"),
        "pipeline commands carry the headless run mode: {runbook}"
    );
    assert!(
        !runbook.contains("{{"),
        "no unsubstituted tokens: {runbook}"
    );
}

#[test]
fn claude_hybrid_record_runs_does_not_require_a_session_id() {
    // Regression: hybrid/headless ride the Cli mechanism and read each task's
    // claude-events.jsonl, never the in-session subagents dir. Resolving that dir
    // is gated on the dispatch mechanism, not the harness, so `record-runs` in
    // hybrid mode must NOT bail on a missing CLAUDE_CODE_SESSION_ID — the way the
    // old harness-keyed gate did for `--harness claude-code`. This is the
    // documented headless path (no session at all).
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
        ])
        .assert()
        .success();

    // No session id in the environment, and none passed — the pre-fix code aborted
    // here with "could not auto-resolve the subagents dir". The fix returns early
    // for the Cli mechanism, so record-runs proceeds to its summary.
    skill_eval()
        .current_dir(&cwd)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .args(["record-runs", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--workspace-dir"])
        .arg(cwd.join("skills-workspace"))
        .args(["--harness", "claude-code", "--run-mode", "hybrid"])
        .assert()
        .success()
        .stdout(contains("Recorded:"));
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
fn claude_cli_guard_installs_project_hook() {
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
        ])
        .assert()
        .success();

    // The guard installs into the isolated env (the agent-under-test's cwd) — the
    // same `.claude/settings.local.json` each `claude -p` dispatch loads from that
    // cwd, so a PreToolUse deny fires under Cli dispatch.
    let settings_path = env_dir(&cwd).join(".claude/settings.local.json");
    assert!(settings_path.exists());
    let settings = read_json(&settings_path);
    let hook = &settings["hooks"]["PreToolUse"][0];
    let command = hook["hooks"][0]["command"].as_str().unwrap();
    assert!(
        command.contains("guard") && !command.contains("guard-codex"),
        "hook invokes the claude guard entry point: {settings}"
    );
    assert!(
        hook["matcher"].as_str().unwrap().contains("Write"),
        "hook matches write tools: {settings}"
    );
    assert!(
        env_dir(&cwd)
            .join(".claude/skills/.slow-powers-eval-guard.json")
            .exists()
    );
}
