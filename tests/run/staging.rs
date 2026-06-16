//! Staging, plan-mode injection, `--stage-name`, and dispatch-prompt rendering.

use crate::helpers::*;
use predicates::str::contains;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn setup_direct_skill(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let skills = root.join("skills");
    let skill_sub = skills.join("mr-review");
    let helper = skills.join("helper-skill");
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    fs::create_dir_all(&helper).unwrap();
    fs::write(
        skill_sub.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review merge requests\n---\n\nbody\n",
    )
    .unwrap();
    fs::write(skill_sub.join("evals").join("evals.json"), DEFAULT_EVALS).unwrap();
    fs::write(
        helper.join("SKILL.md"),
        "---\nname: helper-skill\ndescription: helper\n---\n\nhelper\n",
    )
    .unwrap();
    let cwd = root.join("work");
    fs::create_dir_all(&cwd).unwrap();
    (skills, skill_sub, cwd)
}

fn direct_iteration_dir(cwd: &Path) -> PathBuf {
    cwd.join("skills-workspace")
        .join("mr-review")
        .join("iteration-1")
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
fn run_from_skill_dir_defaults_to_new_skill_without_staging_siblings() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (_skills, skill_sub, _cwd) = setup_direct_skill(tmp.path());

    skill_eval()
        .current_dir(&skill_sub)
        .arg("run")
        .assert()
        .success()
        .stdout(contains("Preparing mr-review iteration-1 (new-skill)"))
        .stdout(contains("eval-magic ingest --skill-dir"))
        .stdout(contains("--skill mr-review --iteration 1"));

    assert!(
        direct_iteration_dir(&skill_sub)
            .join("dispatch.json")
            .exists()
    );
    assert_eq!(
        staged_entries(&skill_sub.join(".claude/skills")),
        vec!["slow-powers-eval-1-with_skill__mr-review"]
    );

    let dispatch = read_json(&direct_iteration_dir(&skill_sub).join("dispatch.json"));
    let task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["condition"] == "with_skill")
        .unwrap();
    let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
    assert!(prompt.contains("- mr-review:"));
    assert!(!prompt.contains("helper-skill"));
}

#[test]
fn run_with_skill_path_defaults_to_single_skill_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (_skills, skill_sub, cwd) = setup_direct_skill(tmp.path());

    skill_eval()
        .current_dir(&cwd)
        .arg("run")
        .arg("--skill")
        .arg(&skill_sub)
        .args(["--dry-run"])
        .assert()
        .success()
        .stdout(contains("Preparing mr-review iteration-1 (new-skill)"));

    assert!(direct_iteration_dir(&cwd).join("dispatch.json").exists());
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
