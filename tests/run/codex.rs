//! Codex-harness behavior: `.agents/skills` staging, inline fallback, guard
//! wiring, bootstrap injection, and live-skill shadow detection.

use crate::helpers::*;
use predicates::str::contains;
use std::fs;
use std::path::Path;

#[test]
fn codex_no_stage_keeps_inline_fallback() {
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
            "codex",
            "--no-stage",
            "--dry-run",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(dispatch["harness"], "codex");
    assert_eq!(conditions["harness"], "codex");
    assert!(!cwd.join(".claude/skills").exists());
    assert!(!cwd.join(".agents/skills").exists());
}

#[test]
fn codex_stages_repo_local_skills_under_agents() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let helper = skill_dir.join("release-notes");
    fs::create_dir_all(&helper).unwrap();
    fs::write(
        helper.join("SKILL.md"),
        "---\nname: release-notes\ndescription: draft release notes\n---\n\nnotes\n",
    )
    .unwrap();
    fs::write(helper.join("helper.md"), "helper guidance").unwrap();

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
            "codex",
            "--dry-run",
        ])
        .assert()
        .success();

    // Codex rides Cli dispatch → per-(group, condition) envs. The skill stages into
    // the with_skill env; the control arm's env carries the siblings but NOT the SUT.
    let slug = "slow-powers-eval-1-with_skill__mr-review";
    let codex_skills = cli_env_dir(&cwd, "g1", "with_skill").join(".agents/skills");
    assert!(read_str(&codex_skills.join(slug).join("SKILL.md")).contains(&format!("name: {slug}")));
    assert_eq!(
        read_str(&codex_skills.join("release-notes/helper.md")),
        "helper guidance"
    );
    assert!(!codex_skills.join("release-notes/evals").exists());
    assert!(!cwd.join(".claude/skills").exists());

    // The gap fix: the control arm's env never contains the skill-under-test.
    let without_skills = cli_env_dir(&cwd, "g1", "without_skill").join(".agents/skills");
    assert!(!without_skills.join(slug).exists());
    assert!(without_skills.join("release-notes/SKILL.md").exists());

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["condition"] == "with_skill")
        .unwrap();
    let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
    assert!(prompt.contains("## Skills"));
    assert!(prompt.contains(&format!("- {slug}: review merge requests")));
    assert!(prompt.contains("- release-notes: draft release notes"));
    assert!(prompt.contains(&format!("identifier `{slug}`")));
    assert!(!prompt.contains("<skill name="));
    assert!(!prompt.contains("The following skills are available for use with the Skill tool:"));
}

#[test]
fn codex_supports_stage_name_when_staging() {
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
            "codex",
            "--stage-name",
            "mr-review",
            "--dry-run",
        ])
        .assert()
        .success();

    assert!(
        read_str(&cli_env_dir(&cwd, "g1", "with_skill").join(".agents/skills/mr-review/SKILL.md"))
            .contains("name: mr-review")
    );
}

#[test]
fn codex_plan_mode_injects_profile_and_records_flag() {
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
            "codex",
            "--plan-mode",
            "--dry-run",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(dispatch["plan_mode"], true);
    for task in dispatch["tasks"].as_array().unwrap() {
        let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
        if task["condition"] == "with_skill" {
            assert!(prompt.contains("## Skills"));
        }
        assert!(prompt.contains("<system-reminder>"));
        // Shared, harness-agnostic profile: same text every harness sees, with no
        // Codex-specific <proposed_plan> block or Claude-specific ExitPlanMode rail.
        assert!(prompt.contains("Plan mode is active"));
        assert!(!prompt.contains("<proposed_plan>"));
        assert!(!prompt.contains("ExitPlanMode"));
    }
}

#[test]
fn codex_guard_installs_project_hook() {
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
            "codex",
            "--guard",
        ])
        .assert()
        .success()
        .stdout(contains("--dangerously-bypass-hook-trust"));

    // The guard installs into each per-(group, condition) env (the agent-under-test's
    // cwd).
    let with_env = cli_env_dir(&cwd, "g1", "with_skill");
    let hooks_path = with_env.join(".codex/hooks.json");
    assert!(hooks_path.exists());
    let hooks = read_json(&hooks_path);
    let hook = &hooks["hooks"]["PreToolUse"][0];
    assert!(
        hook["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("guard-codex")
    );
    assert!(
        with_env
            .join(".agents/skills/.slow-powers-eval-guard.json")
            .exists()
    );
    // The control arm's env is guarded too.
    assert!(
        cli_env_dir(&cwd, "g1", "without_skill")
            .join(".codex/hooks.json")
            .exists()
    );
}

#[test]
fn codex_dispatch_guidance_detaches_stdin_and_logs_stderr() {
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
            "codex",
            "--guard",
        ])
        .assert()
        .success();
    // The post-run hand-off names the runner command; the harness CLI it will
    // spawn is documented in the manifest, not pasted at the operator.
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("eval-magic dispatch"), "{stdout}");
    assert!(stdout.contains("--harness codex"), "{stdout}");

    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(manifest.contains("codex --ask-for-approval never exec --cd <eval-root>"));
    assert!(manifest.contains("--dangerously-bypass-hook-trust"));
    assert!(manifest.contains("</dev/null"));
    assert!(manifest.contains("codex-events.jsonl"));
    assert!(manifest.contains("codex-stderr.log"));
    // Concurrency is the runner's `--jobs`, not a pasted `xargs -P` pipeline.
    assert!(manifest.contains("eval-magic dispatch"));
}

#[test]
fn codex_dispatch_guidance_includes_agent_model_when_provided() {
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
            "codex",
            "--agent-model",
            "gpt-5-mini",
        ])
        .assert()
        .success();
    assert.success();
    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(manifest.contains("codex --ask-for-approval never exec --cd <eval-root>"));
    assert!(manifest.contains("-m gpt-5-mini"));
    assert!(manifest.contains("</dev/null"));
    assert!(manifest.contains("codex-events.jsonl"));
    assert!(manifest.contains("codex-stderr.log"));
    assert!(manifest.contains("eval-magic dispatch"));
}

#[test]
fn codex_dispatch_guidance_omits_hook_bypass_when_unguarded() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // The guard auto-arms on codex, so an unguarded run takes --no-guard.
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
            "codex",
            "--no-guard",
        ])
        .assert()
        .success();
    assert.success();
    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(manifest.contains("codex --ask-for-approval never exec --cd <eval-root>"));
    assert!(manifest.contains("</dev/null"));
    assert!(!manifest.contains("--dangerously-bypass-hook-trust"));
}

#[test]
fn codex_default_run_auto_arms_guard() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // No guard flag: codex declares guard support, so the bare run arms it and
    // the dispatch recipe carries the hook-trust bypass the staged hook needs.
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
            "codex",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("--dangerously-bypass-hook-trust"));

    assert!(
        cli_env_dir(&cwd, "g1", "with_skill")
            .join(".codex/hooks.json")
            .exists(),
        "codex guard hook staged in the env"
    );
}

#[test]
fn codex_run_writes_human_followed_runbook() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "codex", "--dry-run"])
        .assert()
        .success();

    // Each task dispatches from its own per-(group, condition) env, so the
    // human-followed runbook lives in the iteration dir, above those envs.
    let runbook = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    assert!(
        runbook.contains("human driving"),
        "uses the human-followed template: {runbook}"
    );
    assert!(
        runbook.contains("eval-magic dispatch"),
        "carries the dispatch command: {runbook}"
    );
}

#[test]
fn codex_rejects_stage_name_with_no_stage() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(&tmp.path().join("c-stage-name"), DEFAULT_EVALS);
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
            "codex",
            "--no-stage",
            "--stage-name",
            "natural-name",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(contains("--stage-name"));
}

#[test]
fn codex_no_stage_supports_bootstrap() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let bootstrap = cwd.join("bootstrap.md");
    fs::write(&bootstrap, "BOOT").unwrap();
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
            "codex",
            "--no-stage",
            "--bootstrap",
        ])
        .arg(&bootstrap)
        .arg("--dry-run")
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
        assert!(prompt.contains("<session-start-context>"));
        assert!(prompt.contains("BOOT"));
        if task["condition"] == "with_skill" {
            assert!(prompt.contains("<skill name=\"mr-review\">"));
        }
    }
    assert!(!cwd.join(".agents/skills").exists());
}

#[test]
fn codex_warns_when_user_skill_shadows_staged_skill() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
      { "id": "e1", "prompt": "p1", "expected_output": "o", "files": ["a.txt"] },
      { "id": "e2", "prompt": "p2", "expected_output": "o", "files": ["b.txt"],
        "isolation": "isolated" }
    ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    fs::write(skill_dir.join("mr-review/evals/a.txt"), "a").unwrap();
    fs::write(skill_dir.join("mr-review/evals/b.txt"), "b").unwrap();
    let fake_home = tmp.path().join("home");
    let live_skill = fake_home.join(".agents/skills/different-folder");
    fs::create_dir_all(&live_skill).unwrap();
    fs::write(
        live_skill.join("SKILL.md"),
        "---\nname: mr-review\ndescription: installed copy\n---\n\nlive\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .env("HOME", &fake_home)
        .env("CODEX_HOME", tmp.path().join("codex-home"))
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "codex", "--dry-run"])
        .assert()
        .success()
        .stderr(contains("Skill-shadow preflight"))
        // Pre-dispatch the banner states the stake conditionally. Codex
        // transcripts carry no skill/plugin roster, so it also says the verdict
        // cannot be settled from them rather than promising verification.
        .stderr(contains("would invalidate the comparison if loaded"))
        .stderr(contains("do not report the session's skill/plugin surface"))
        .stderr(contains("Move or rename"));

    let report = read_json(&iteration_dir(&cwd).join("plugin-shadow.json"));
    assert_eq!(report["schema_version"], 3);
    assert_eq!(report["findings"][0]["skill_name"], "mr-review");
    assert_eq!(report["findings"][0]["role"], "subject");
    let sources = report["findings"][0]["sources"].as_array().unwrap();
    let live = sources
        .iter()
        .find(|source| source["origin"] == "live")
        .unwrap();
    assert_eq!(live["kind"], "skill");
    assert_eq!(live["root"]["namespace"], "agents");
    assert_eq!(live["discovery_path"], wire_path(&live_skill).as_str());
    let appearances = live["appearances"].as_array().unwrap();
    assert_eq!(
        appearances.len(),
        4,
        "every group/condition cell is scanned"
    );
    assert!(appearances.iter().any(|cell| {
        cell["group"] == "g2"
            && cell["condition"] == "without_skill"
            && cell["eval_ids"] == serde_json::json!(["e2"])
    }));
}
