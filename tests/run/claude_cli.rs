//! Claude Code CLI dispatch: `claude -p` stream-json dispatch guidance, the
//! human-followed runbook, and the write guard under CLI dispatch.

use crate::helpers::*;
use predicates::str::contains;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn claude_dispatch_guidance_uses_claude_p() {
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
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    // The post-run hand-off names the runner command; the harness CLI it will
    // spawn is documented in the manifest, not pasted at the operator.
    assert!(stdout.contains("eval-magic dispatch"), "{stdout}");
    assert!(stdout.contains("--harness claude-code"), "{stdout}");

    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(manifest.contains("claude -p --output-format stream-json"));
    assert!(manifest.contains("--verbose"));
    assert!(manifest.contains("cd <eval-root>"));
    assert!(manifest.contains("claude-events.jsonl"));
    assert!(!manifest.contains("--output-last-message"));
    // Concurrency is the runner's `--jobs`, not a pasted `xargs -P` pipeline.
    assert!(manifest.contains("eval-magic dispatch"));

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(conditions["harness"], "claude-code");
}

#[test]
fn claude_dispatch_guidance_includes_agent_model_when_provided() {
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
            "--agent-model",
            "opus",
        ])
        .assert()
        .success();
    assert.success();
    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(manifest.contains("claude -p --output-format stream-json"));
    assert!(manifest.contains("--model opus"), "{manifest}");
}

#[test]
fn claude_run_writes_human_followed_runbook() {
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

    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(manifest.contains("claude -p --output-format stream-json"));

    // Each task dispatches from its own per-(group, condition) env, so the shared
    // human-followed runbook lives in the iteration dir, above those envs, and
    // carries the runner commands with --harness threaded through them.
    let runbook = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    assert!(
        runbook.contains("human driving"),
        "uses the human-followed template: {runbook}"
    );
    assert!(
        runbook.contains("eval-magic dispatch"),
        "carries the dispatch command: {runbook}"
    );
    assert!(
        runbook.contains("--harness claude-code"),
        "pipeline commands carry --harness claude-code: {runbook}"
    );
    assert!(
        !runbook.contains("{{"),
        "no unsubstituted tokens: {runbook}"
    );
}

#[test]
fn claude_record_runs_does_not_require_a_session_id() {
    // Regression: CLI dispatch reads each task's claude-events.jsonl, never an
    // in-session subagents dir, so `record-runs --harness claude-code` must NOT
    // bail on a missing CLAUDE_CODE_SESSION_ID.
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "claude-code"])
        .assert()
        .success();

    // No session id in the environment, and none passed — record-runs proceeds to
    // its summary rather than aborting on an unresolved subagents dir.
    skill_eval()
        .current_dir(&cwd)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .args(["record-runs", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--workspace-dir"])
        .arg(cwd.join(".eval-magic"))
        .args(["--harness", "claude-code"])
        .assert()
        .success()
        .stdout(contains("Recorded:"));
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
        .args(["--skill", "mr-review", "--harness", "claude-code"])
        .assert()
        .success();

    assert!(
        iteration_dir(&cwd).join("plugin-shadow.json").exists(),
        "preflight detected the project-enabled plugin shadow by scanning the staged env"
    );
    let artifact = read_json(&iteration_dir(&cwd).join("plugin-shadow.json"));
    assert_eq!(artifact["schema_version"], 3);
    assert!(
        artifact.get("isolates_live_sources").is_none(),
        "false isolation assertions stay omitted"
    );
}

#[test]
fn declared_shadow_isolation_records_findings_as_informational_provenance() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let config = tmp.path().join("config");
    fs::create_dir_all(config.join("skills/mr-review")).unwrap();
    fs::write(
        config.join("skills/mr-review/SKILL.md"),
        "---\nname: mr-review\ndescription: live copy\n---\n",
    )
    .unwrap();
    let overlay = tmp.path().join("isolated.toml");
    fs::write(
        &overlay,
        "label = \"claude-code\"\n\n[shadow]\nisolates_live_sources = true\n",
    )
    .unwrap();

    let assert = skill_eval()
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &config)
        .arg("--harness-file")
        .arg(&overlay)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "claude-code"])
        .assert()
        .success();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("Skill-shadow notice"), "{stderr}");
    assert!(stderr.contains("isolates_live_sources = true"), "{stderr}");
    assert!(!stderr.contains("Plugin-shadow warning"), "{stderr}");

    let artifact = read_json(&iteration_dir(&cwd).join("plugin-shadow.json"));
    assert_eq!(artifact["schema_version"], 3);
    assert_eq!(artifact["isolates_live_sources"], true);
    assert_eq!(artifact["findings"][0]["skill_name"], "mr-review");
    assert_eq!(artifact["findings"][0]["severity"], "comparison-invalid");
}

/// The preflight has not dispatched anything yet, so it must not convict the
/// run. This is the surface that made issue #207 expensive: a correctly-isolated
/// campaign was told its comparison was invalid before a single task ran.
#[test]
fn the_shadow_preflight_banner_does_not_assert_a_verdict_before_dispatch() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let config = tmp.path().join("config");
    fs::create_dir_all(config.join("skills/mr-review")).unwrap();
    fs::write(
        config.join("skills/mr-review/SKILL.md"),
        "---\nname: mr-review\ndescription: live copy\n---\n",
    )
    .unwrap();

    let assert = skill_eval()
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &config)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "claude-code"])
        .assert()
        .success();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("Skill-shadow preflight"), "{stderr}");
    assert!(
        stderr.contains("would invalidate the comparison if loaded"),
        "the stake is stated conditionally: {stderr}"
    );
    assert!(
        !stderr.contains("[comparison invalid]"),
        "no verdict before any dispatch ran: {stderr}"
    );
    assert!(
        stderr.contains("`ingest` records what each dispatch actually loaded"),
        "Claude Code can settle this from transcripts, and should say so: {stderr}"
    );
}

/// End-to-end for issue #207: captures reporting an empty plugin list refute the
/// finding, and the `comparison invalid` warning disappears from the benchmark.
///
/// The collision is plugin-sourced, mirroring the campaign that produced the
/// ticket. That matters: a plugin skill is advertised as `<plugin>:<skill>`,
/// which can never be confused with the staged copy, so the transcript is
/// decisive. A direct global skill is also distinguishable from the uniquely
/// slugged staged subject — covered by the staged runtime-ID integration tests.
#[test]
fn ingest_refutes_a_shadow_finding_when_dispatches_report_an_empty_surface() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let config = tmp.path().join("config");
    let install = config.join("plugins/cache/shadowplug__test");
    fs::create_dir_all(install.join("skills/mr-review")).unwrap();
    fs::write(
        install.join("skills/mr-review/SKILL.md"),
        "---\nname: mr-review\ndescription: live copy\n---\n",
    )
    .unwrap();
    fs::write(
        config.join("plugins/installed_plugins.json"),
        format!(
            "{{\"version\":2,\"plugins\":{{\"shadowplug@test\":[{{\"installPath\":{:?}}}]}}}}",
            install.to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(
        config.join("settings.json"),
        "{\"enabledPlugins\":{\"shadowplug@test\":true}}",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &config)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "claude-code"])
        .assert()
        .success();

    let iteration = iteration_dir(&cwd);
    let dispatch = read_json(&iteration.join("dispatch.json"));
    // Every dispatch reports an init event whose rosters are empty: the live
    // `mr-review` copy was not discoverable from any of them.
    for task in dispatch["tasks"].as_array().unwrap() {
        let outputs = PathBuf::from(task["outputs_dir"].as_str().unwrap());
        fs::create_dir_all(&outputs).unwrap();
        fs::write(
            outputs.join("claude-events.jsonl"),
            "{\"type\":\"system\",\"subtype\":\"hook_started\"}\n\
             {\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\",\
              \"plugins\":[],\"skills\":[]}\n\
             {\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\
              \"result\":\"done\",\"duration_ms\":5,\"usage\":{\"input_tokens\":1,\
              \"output_tokens\":1}}\n",
        )
        .unwrap();
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["record-runs", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "claude-code"])
        .assert()
        .success();

    let surface = read_json(&iteration.join("session-surface.json"));
    assert!(surface["tasks_with_evidence"].as_u64().unwrap() > 0);
    assert_eq!(surface["tasks_without_evidence"], 0);

    let artifact = read_json(&iteration.join("plugin-shadow.json"));
    assert_eq!(artifact["findings"][0]["resolved_severity"], "isolated");
    assert_eq!(
        artifact["findings"][0]["severity"], "comparison-invalid",
        "the intrinsic severity records the risk and is never rewritten"
    );
    assert_eq!(artifact["verification"]["refuted_findings"], 1);
    assert_eq!(artifact["verification"]["confirmed_findings"], 0);
}

#[test]
fn run_omits_run_mode_from_every_artifact_and_command() {
    // The run-mode vocabulary is retired: there is one CLI dispatch path, so no
    // artifact records a run mode and no printed/threaded command carries the flag.
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
            "--dry-run",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        !stdout.contains("--run-mode"),
        "printed next-step commands carry no --run-mode: {stdout}"
    );

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert!(
        conditions.get("run_mode").is_none(),
        "conditions.json carries no run_mode: {conditions}"
    );
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert!(
        dispatch.get("run_mode").is_none(),
        "dispatch.json carries no run_mode: {dispatch}"
    );
    let runbook = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    assert!(
        !runbook.contains("--run-mode"),
        "runbook pipeline commands carry no --run-mode: {runbook}"
    );
    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(
        !manifest.contains("--run-mode"),
        "dispatch manifest carries no --run-mode: {manifest}"
    );
}

#[test]
fn run_mode_flag_is_rejected() {
    // `--run-mode` is fully removed, not a hidden no-op: clap rejects it.
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
        .failure();
}

#[test]
fn claude_ingest_reports_permission_denied_tool_calls() {
    // #180: a refused tool call still appears in the transcript and the dispatch
    // still exits 0, so without this report a run that degraded to static
    // reasoning grades as if it had executed something.
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "claude-code"])
        .assert()
        .success();

    // Simulate the dispatches: the with_skill arm had its repro command refused,
    // the without_skill arm ran cleanly.
    let tasks = read_json(&iteration_dir(&cwd).join("dispatch.json"))["tasks"]
        .as_array()
        .expect("dispatch.json carries tasks[]")
        .clone();
    assert_eq!(tasks.len(), 2, "{tasks:?}");
    for task in &tasks {
        let outputs = Path::new(task["outputs_dir"].as_str().unwrap()).to_path_buf();
        let outputs = if outputs.is_absolute() {
            outputs
        } else {
            cwd.join(outputs)
        };
        fs::create_dir_all(&outputs).unwrap();
        fs::write(outputs.join("final-message.md"), "Reviewed.\n").unwrap();
        let refused = task["condition"].as_str() == Some("with_skill");
        let denials = if refused {
            r#","permission_denials":[{"tool_name":"Bash","tool_use_id":"toolu_1","tool_input":{"command":"TZ=UTC bun run repro.ts","description":"repro"}}]"#
        } else {
            ""
        };
        fs::write(
            outputs.join("claude-events.jsonl"),
            format!(
                concat!(
                    r#"{{"type":"assistant","message":{{"id":"msg_1","role":"assistant","content":[{{"type":"tool_use","id":"toolu_1","name":"Bash","input":{{"command":"TZ=UTC bun run repro.ts"}}}}]}}}}"#,
                    "\n",
                    r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"This command requires approval","is_error":true}}]}}}}"#,
                    "\n",
                    r#"{{"type":"result","subtype":"success","is_error":false,"result":"Reviewed.","duration_ms":12,"usage":{{"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}{denials}}}"#,
                    "\n",
                ),
                denials = denials
            ),
        )
        .unwrap();
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["record-runs", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--workspace-dir"])
        .arg(cwd.join(".eval-magic"))
        .args(["--harness", "claude-code", "--iteration", "1"])
        .assert()
        .success()
        .stderr(contains("permission-denied"))
        .stderr(contains("permission-denials.json"));

    // One task, one refusal — the clean arm contributes nothing.
    let report = read_json(&iteration_dir(&cwd).join("permission-denials.json"));
    assert_eq!(report["iteration"], 1, "{report}");
    assert_eq!(report["total_denials"], 1, "{report}");
    let reported = report["tasks"].as_array().unwrap();
    assert_eq!(reported.len(), 1, "{report}");
    assert_eq!(reported[0]["condition"], "with_skill");
    assert_eq!(reported[0]["guard_attributed_count"], 0);
    assert_eq!(reported[0]["denials"][0]["tool"], "Bash");
    assert_eq!(
        reported[0]["denials"][0]["reason"],
        "This command requires approval"
    );
    // Privacy-safe like guard-denials.json: keys, never the values.
    assert_eq!(
        reported[0]["denials"][0]["input_keys"],
        serde_json::json!(["command", "description"])
    );

    // Grading is untouched — both arms still produced run records.
    for task in &tasks {
        let record = Path::new(task["run_record_path"].as_str().unwrap()).to_path_buf();
        let record = if record.is_absolute() {
            record
        } else {
            cwd.join(record)
        };
        assert!(record.exists(), "{record:?}");
    }
}
