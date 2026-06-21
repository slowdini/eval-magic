//! Claude Code CLI run modes (`--run-mode hybrid` / `headless`): `claude -p`
//! stream-json dispatch guidance, run-mode persistence + defaulting, the
//! human-followed runbook, the write guard under Cli dispatch, and the remaining
//! run-mode combo rejections (Codex interactive).

use crate::helpers::*;
use predicates::str::contains;
use std::fs;

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

    // Cli dispatches from per-(group, condition) envs, so the human-followed
    // runbook lives in the iteration dir, not a single env/.
    let runbook = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
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
    // recipe and headless-threaded pipeline commands. Cli has no single env/, so
    // it lives in the iteration dir.
    let runbook = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
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

    // The guard installs into EACH per-(group, condition) env (the agent-under-test's
    // cwd) — the same `.claude/settings.local.json` each `claude -p` dispatch loads
    // from that cwd, so a PreToolUse deny fires under Cli dispatch.
    let with_env = cli_env_dir(&cwd, "g1", "with_skill");
    let settings_path = with_env.join(".claude/settings.local.json");
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
        with_env
            .join(".claude/skills/.slow-powers-eval-guard.json")
            .exists()
    );

    // The control arm's env is independently guarded too, and — the gap fix — holds
    // no staged skill slug at all (the skill is physically absent, not just unlisted).
    let without_env = cli_env_dir(&cwd, "g1", "without_skill");
    assert!(
        without_env.join(".claude/settings.local.json").exists(),
        "the without_skill env is guarded too"
    );
    assert!(
        !without_env
            .join(".claude/skills/slow-powers-eval-1-with_skill__mr-review")
            .exists(),
        "the control arm's env contains no staged skill slug"
    );
}

#[test]
fn cli_plugin_shadow_preflight_reads_per_env_project_settings() {
    let tmp = tempfile::TempDir::new().unwrap();
    // The eval stages a project-local `.claude/settings.json` into its env (fixture).
    let evals = r#"{ "skill_name": "mr-review", "evals": [ { "id": "e1", "prompt": "p", "expected_output": "o", "files": [".claude/settings.json"] } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);

    // A Claude config dir whose installed plugin provides a skill named like the SUT,
    // but the plugin is NOT enabled at config level — only the project-local
    // `.claude/settings.json` (staged into each env as a fixture) enables it. So the
    // preflight can only see the override when it scans the real staged env; under Cli
    // the legacy `env/` is never created, which is the bug this locks down.
    let config = tmp.path().join("config");
    let install = config.join("plugins/cache/shadowplug__test");
    fs::create_dir_all(install.join("skills/mr-review")).unwrap();
    fs::write(
        install.join("skills/mr-review/SKILL.md"),
        "---\nname: mr-review\ndescription: x\n---\n",
    )
    .unwrap();
    fs::create_dir_all(config.join("plugins")).unwrap();
    fs::write(
        config.join("plugins/installed_plugins.json"),
        format!(
            "{{\"version\":2,\"plugins\":{{\"shadowplug@test\":[{{\"installPath\":{:?}}}]}}}}",
            install.to_string_lossy()
        ),
    )
    .unwrap();

    // The fixture that, once staged into the env, enables the plugin project-locally.
    // (No config-level settings.json — the plugin is enabled ONLY via the env's file.)
    fs::create_dir_all(skill_dir.join("mr-review/evals/.claude")).unwrap();
    fs::write(
        skill_dir.join("mr-review/evals/.claude/settings.json"),
        "{\"enabledPlugins\":{\"shadowplug@test\":true}}",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &config)
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

    assert!(
        iteration_dir(&cwd).join("plugin-shadow.json").exists(),
        "preflight detected the project-enabled plugin shadow by scanning the staged env"
    );
}
