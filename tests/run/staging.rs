//! Staging, `--stage-name`, and dispatch-prompt rendering.

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
    let codebase = root.join("codebase");
    fs::create_dir_all(&codebase).unwrap();
    fs::write(codebase.join("README.md"), "# Test codebase\n").unwrap();
    let mut evals: Value = serde_json::from_str(DEFAULT_EVALS).unwrap();
    evals["codebase"] = serde_json::json!({ "path": codebase.to_string_lossy() });
    fs::write(
        skill_sub.join("evals").join("evals.json"),
        serde_json::to_string_pretty(&evals).unwrap(),
    )
    .unwrap();
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
    cwd.join(".eval-magic")
        .join("mr-review")
        .join("iteration-1")
}

/// The relocation's acceptance criterion, end to end: a run started from inside
/// a skills repository leaves nothing behind in it. `XDG_DATA_HOME` stands in
/// for the operator's data directory so the derived default — slug and all — is
/// the thing under test rather than something the harness pinned.
#[test]
fn a_run_from_inside_a_skills_repo_writes_no_workspace_into_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (_skills, skill_sub, _cwd) = setup_direct_skill(tmp.path());
    let data_home = tmp.path().join("xdg-data");

    skill_eval()
        .env_remove("EVAL_MAGIC_WORKSPACE_DIR")
        .env("XDG_DATA_HOME", &data_home)
        .current_dir(&skill_sub)
        .args(["run", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    assert!(
        !skill_sub.join(".eval-magic").exists(),
        "the skill directory holds a workspace"
    );
    assert!(
        !tmp.path().join("skills").join(".eval-magic").exists(),
        "the skills repository holds a workspace"
    );

    let roots: Vec<PathBuf> = fs::read_dir(data_home.join("eval-magic"))
        .expect("the derived eval home was created")
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(
        roots.len(),
        1,
        "expected one per-source root, got {roots:?}"
    );
    assert!(
        roots[0].join("mr-review").join("iteration-1").exists(),
        "iteration missing under {}",
        roots[0].display()
    );

    let slug = roots[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        slug.starts_with("skills-") && slug.len() > "skills-".len(),
        "root should be namespaced by the skill directory, was {slug}"
    );
}

#[test]
fn stages_only_sut_and_writes_workspace_under_the_configured_home() {
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
        env_staged_entries(&cwd),
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
        // The run summary points at the human-followed RUNBOOK (a copy of the dispatch
        // steps); the auto-derived pipeline commands are threaded into it (asserted below).
        .stdout(contains("RUNBOOK.md"));

    assert!(
        direct_iteration_dir(&skill_sub)
            .join("dispatch.json")
            .exists()
    );
    assert_eq!(
        env_staged_entries(&skill_sub),
        vec!["slow-powers-eval-1-with_skill__mr-review"]
    );

    // Run from inside the skill dir with no args: the auto-derived target selector
    // (`command_target_args`) is threaded into the RUNBOOK's pipeline commands. The
    // RUNBOOK lives in the iteration dir (Cli dispatch has no single env/). The
    // invocation used no --skill-dir, so the selector must not invent one:
    // --skill-dir stages sibling skills, changing the experiment (#294).
    let runbook = read_str(&direct_iteration_dir(&skill_sub).join("RUNBOOK.md"));
    assert!(
        !runbook.contains("--skill-dir"),
        "the selector must not add --skill-dir the invocation never used: {runbook}"
    );
    assert!(
        runbook.contains(&format!(
            "ingest --skill {}",
            wire_path(&resolved(&skill_sub))
        )),
        "the selector names --skill as an absolute path: {runbook}"
    );
    assert!(runbook.contains("--iteration 1"));

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
        env_staged_entries(&cwd),
        vec!["slow-powers-eval-1-with_skill__mr-review"]
    );
}

#[test]
fn dispatch_carries_no_run_level_plan_flag_and_no_system_reminder_block() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    // Plan mode is a per-eval declaration that starts the harness's native
    // plan mode (`eval-magic docs conversations`); nothing about it is a
    // run-level flag or a prompt injection any more.
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert!(dispatch.get("plan_mode").is_none());
    for task in dispatch["tasks"].as_array().unwrap() {
        let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
        assert!(!prompt.contains("<system-reminder>"));
        assert!(!prompt.contains("Plan mode is active"));
    }
}

#[test]
fn the_simulated_plan_mode_flag_is_gone() {
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
        .failure()
        .stderr(contains("--plan-mode"));
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

    let skills_dir = cli_env_dir(&cwd, "g1", "with_skill").join(".claude/skills");
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
