//! End-to-end integration tests for the `run` orchestrator and `teardown`,
//! driving the built `skill-eval` binary against an isolated CWD. Ports the
//! "run.ts user-mode end-to-end" subprocess tests from eval-runner's
//! `run.test.ts`.
//!
//! Unlike the TS original (where bare flags imply `run`), clap owns dispatch, so
//! a flagged invocation names the `run` subcommand explicitly; a bare
//! `skill-eval` with no args still defaults to `run`.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::str::contains;
use serde_json::Value;

const STAGED_MANIFEST: &str = ".slow-powers-eval-manifest.json";
const DEFAULT_EVALS: &str = r#"{ "skill_name": "mr-review", "evals": [ { "id": "e1", "prompt": "review this MR", "expected_output": "a review" } ] }"#;

fn skill_eval() -> Command {
    Command::cargo_bin("skill-eval").expect("binary `skill-eval` should build")
}

/// Build `<root>/skill-dir/mr-review/{SKILL.md,evals/evals.json}` and a `work`
/// cwd; returns `(skill_dir, cwd)`.
fn setup(root: &Path, evals_json: &str) -> (PathBuf, PathBuf) {
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

fn iteration_dir(cwd: &Path) -> PathBuf {
    cwd.join("skills-workspace")
        .join("mr-review")
        .join("iteration-1")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn read_str(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// Names directly under `.claude/skills` (or `.agents/skills`), excluding the
/// staging manifest, sorted.
fn staged_entries(skills_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(skills_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != STAGED_MANIFEST)
        .collect();
    names.sort();
    names
}

#[test]
fn stages_only_sut_and_writes_workspace_under_cwd() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    assert!(iteration_dir(&cwd).join("dispatch.json").exists());
    assert_eq!(
        staged_entries(&cwd.join(".claude/skills")),
        vec!["slow-powers-eval-1-with_skill__mr-review"]
    );
}

#[test]
fn plan_mode_injects_profile_and_records_flag() {
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
            "--plan-mode",
            "--dry-run",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(dispatch["plan_mode"], Value::Bool(true));
    for task in dispatch["tasks"].as_array().unwrap() {
        let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
        assert!(prompt.contains("<system-reminder>"));
        assert!(prompt.contains("Plan mode is active"));
        assert!(prompt.contains("ExitPlanMode"));
    }
}

#[test]
fn without_plan_mode_records_false_and_omits_block() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(dispatch["plan_mode"], Value::Bool(false));
    for task in dispatch["tasks"].as_array().unwrap() {
        let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
        assert!(!prompt.contains("<system-reminder>"));
    }
}

#[test]
fn stage_name_threads_verbatim_name_and_registers_cleanup() {
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
            "--stage-name",
            "mr-review",
            "--dry-run",
        ])
        .assert()
        .success();

    let skills_dir = cwd.join(".claude/skills");
    assert_eq!(staged_entries(&skills_dir), vec!["mr-review"]);

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    let with_skill = conditions["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "with_skill")
        .unwrap();
    assert_eq!(with_skill["staged_skill_slug"], "mr-review");

    let manifest = read_json(&skills_dir.join(STAGED_MANIFEST));
    let names: Vec<&str> = manifest["created_entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"mr-review"));

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["condition"] == "with_skill")
        .unwrap();
    let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
    assert!(prompt.contains("registered under the identifier `mr-review`"));
    assert!(!prompt.contains("slow-powers-eval-"));
}

#[test]
fn stage_name_refuses_to_clobber_preexisting_dir() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let preexisting = cwd.join(".claude/skills/my-real-skill");
    fs::create_dir_all(&preexisting).unwrap();
    fs::write(preexisting.join("SKILL.md"), "USER OWNED").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--stage-name",
            "my-real-skill",
            "--dry-run",
        ])
        .assert()
        .failure();

    assert_eq!(read_str(&preexisting.join("SKILL.md")), "USER OWNED");
}

#[test]
fn dispatch_prompt_lists_only_sut_without_bootstrap() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["condition"] == "with_skill")
        .unwrap();
    // The full prompt lives in a file, not inlined in dispatch.json.
    assert!(task.get("dispatch_prompt").is_none());
    let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
    assert!(prompt.contains("The following skills are available for use with the Skill tool:"));
    assert!(prompt.contains("- mr-review:"));
    assert!(!prompt.contains("test-driven-development"));
    assert!(!prompt.contains("writing-skills"));
    assert!(!prompt.contains("EXTREMELY-IMPORTANT"));
    assert!(!prompt.contains("loaded at session start"));
}

#[test]
fn writes_each_prompt_to_file_and_drops_inline() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let tasks = dispatch["tasks"].as_array().unwrap();
    assert!(!tasks.is_empty());
    for task in tasks {
        assert!(task.get("dispatch_prompt").is_none());
        let path = task["dispatch_prompt_path"].as_str().unwrap();
        assert!(path.ends_with("dispatch-prompt.txt"));
        let contents = read_str(Path::new(path));
        assert!(!contents.is_empty());
        assert!(contents.contains("User request:"));
    }
}

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

    let slug = "slow-powers-eval-1-with_skill__mr-review";
    let codex_skills = cwd.join(".agents/skills");
    assert!(read_str(&codex_skills.join(slug).join("SKILL.md")).contains(&format!("name: {slug}")));
    assert_eq!(
        read_str(&codex_skills.join("release-notes/helper.md")),
        "helper guidance"
    );
    assert!(!codex_skills.join("release-notes/evals").exists());
    assert!(!cwd.join(".claude/skills").exists());

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

    assert!(read_str(&cwd.join(".agents/skills/mr-review/SKILL.md")).contains("name: mr-review"));
}

#[test]
fn codex_rejects_unsupported_parity_features() {
    let tmp = tempfile::TempDir::new().unwrap();

    for extra in [["--guard"].as_slice(), ["--plan-mode"].as_slice()] {
        let (skill_dir, cwd) = setup(&tmp.path().join(format!("c{}", extra[0])), DEFAULT_EVALS);
        let mut cmd = skill_eval();
        cmd.current_dir(&cwd)
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
            .args(extra)
            .assert()
            .failure()
            .stderr(contains("Codex"));
    }

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

    let (skill_dir, cwd) = setup(&tmp.path().join("c-bootstrap"), DEFAULT_EVALS);
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
        .failure()
        .stderr(contains("Codex"));
}

#[test]
fn guard_installs_pretooluse_hook_and_teardown_guard_removes_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let settings = cwd.join(".claude/settings.local.json");

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();
    assert!(settings.exists());
    let parsed = read_json(&settings);
    assert!(
        parsed["hooks"]["PreToolUse"][0]["matcher"]
            .as_str()
            .unwrap()
            .contains("Write")
    );

    skill_eval()
        .current_dir(&cwd)
        .args(["teardown-guard", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success();
    assert!(!settings.exists());
}

#[test]
fn teardown_removes_guard_and_staged_skill_set() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let settings = cwd.join(".claude/settings.local.json");
    let staged = cwd.join(".claude/skills");

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();
    assert!(settings.exists());
    assert!(staged.exists());

    skill_eval()
        .current_dir(&cwd)
        .args(["teardown", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success();
    assert!(!settings.exists());
    assert!(!staged.exists());
    assert!(!cwd.join(".claude").exists());
    assert!(!cwd.join("skills-workspace").exists());
}

#[test]
fn teardown_preserves_iteration_with_uncommitted_results() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill"])
        .assert()
        .success();

    // Simulate a graded-but-not-promoted run.
    fs::write(
        iteration_dir(&cwd).join("benchmark.json"),
        "{\"delta\":{\"pass_rate\":0.4}}\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["teardown", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success()
        .stderr(contains("iteration-1"))
        .stderr(contains("promote-baseline"));

    assert!(iteration_dir(&cwd).exists());
}

#[test]
fn normal_run_does_not_install_guard() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();
    assert!(!cwd.join(".claude/settings.local.json").exists());
}

#[test]
fn namespaces_agent_description_and_records_run_nonce() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let nonce = dispatch["run_nonce"].as_str().unwrap();
    assert!(!nonce.is_empty());
    for task in dispatch["tasks"].as_array().unwrap() {
        let condition = task["condition"].as_str().unwrap();
        let desc = task["agent_description"].as_str().unwrap();
        assert!(desc.ends_with(&format!(":{condition}:i1-{nonce}")));
    }
    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(conditions["run_nonce"].as_str().unwrap(), nonce);
}

#[test]
fn bootstrap_content_prepended_before_available_skills() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let bootstrap = cwd.join("my-bootstrap.md");
    fs::write(&bootstrap, "MY CUSTOM EVAL FRAMING").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--bootstrap"])
        .arg(&bootstrap)
        .arg("--dry-run")
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["condition"] == "with_skill")
        .unwrap();
    let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
    let boot_idx = prompt.find("MY CUSTOM EVAL FRAMING").unwrap();
    let list_idx = prompt
        .find("The following skills are available for use with the Skill tool:")
        .unwrap();
    assert!(list_idx > boot_idx);
}

#[test]
fn only_restricts_dispatches_to_named_ids() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review MR 1", "expected_output": "a review" },
        { "id": "e2", "prompt": "review MR 2", "expected_output": "a review" } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--only",
            "e1",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(contains("1 evals × 2 conditions"));

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let ids: Vec<&str> = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["eval_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["e1", "e1"]);
}

#[test]
fn only_with_unknown_id_exits_nonzero() {
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
            "--only",
            "nope",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(contains("unknown eval id(s): nope"));
}
