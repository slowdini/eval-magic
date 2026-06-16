//! OpenCode-harness behavior: `.opencode/skills` staging, slug sanitization,
//! native `<available_skills>` dispatch rendering, plan-mode approximation, and
//! the `--guard` rejection. Characterization tests pinning current behavior so
//! the run-mode refactor stays behavior-preserving.

use crate::helpers::*;
use predicates::str::contains;
use std::fs;
use std::path::Path;

/// The sanitized slug the staged skill-under-test gets for `(iteration 1,
/// with_skill, mr-review)`: underscores in the condition become hyphens.
const OPENCODE_SLUG: &str = "slow-powers-eval-1-with-skill-mr-review";

#[test]
fn opencode_no_stage_keeps_inline_fallback() {
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
            "opencode",
            "--no-stage",
            "--dry-run",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(dispatch["harness"], "opencode");
    assert_eq!(conditions["harness"], "opencode");
    assert!(!cwd.join(".claude/skills").exists());
    assert!(!cwd.join(".agents/skills").exists());
    assert!(!cwd.join(".opencode/skills").exists());
}

#[test]
fn opencode_stages_repo_local_skills_under_opencode() {
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
            "opencode",
            "--dry-run",
        ])
        .assert()
        .success();

    let opencode_skills = cwd.join(".opencode/skills");
    assert!(
        read_str(&opencode_skills.join(OPENCODE_SLUG).join("SKILL.md"))
            .contains(&format!("name: {OPENCODE_SLUG}"))
    );
    assert_eq!(
        read_str(&opencode_skills.join("release-notes/helper.md")),
        "helper guidance"
    );
    assert!(!opencode_skills.join("release-notes/evals").exists());
    assert!(!cwd.join(".claude/skills").exists());
    assert!(!cwd.join(".agents/skills").exists());

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["condition"] == "with_skill")
        .unwrap();
    let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
    // OpenCode's native skill surface: an <available_skills> XML block. Current
    // behavior advertises the skill-under-test under its NATURAL name (only Codex
    // advertises the slug — see build.rs available_skills_for), even though the
    // OpenCode-staged frontmatter name: is rewritten to the slug. Pinning that as-is.
    assert!(prompt.contains("<available_skills>"));
    assert!(prompt.contains("</available_skills>"));
    assert!(prompt.contains("<name>mr-review</name>"));
    assert!(prompt.contains("<description>review merge requests</description>"));
    assert!(prompt.contains("<name>release-notes</name>"));
    assert!(prompt.contains("<description>draft release notes</description>"));
    // Neutral slug-disambiguation framing for OpenCode points at the real identifier.
    assert!(prompt.contains(&format!("identifier `{OPENCODE_SLUG}`")));
    assert!(prompt.contains("as an OpenCode skill"));
    // Must not leak the Claude Code or Codex skill surfaces.
    assert!(!prompt.contains("The following skills are available for use with the Skill tool:"));
    assert!(!prompt.contains("## Skills"));
}

#[test]
fn opencode_plan_mode_injects_profile_and_records_flag() {
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
            "opencode",
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
            assert!(prompt.contains("<available_skills>"));
        }
        assert!(prompt.contains("<system-reminder>"));
        assert!(prompt.contains("OpenCode plan mode is active"));
        assert!(!prompt.contains("ExitPlanMode"));
    }
}

#[test]
fn opencode_rejects_guard() {
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
            "opencode",
            "--guard",
        ])
        .assert()
        .failure()
        .stderr(contains("not yet supported for the opencode harness"));
}

#[test]
fn opencode_rejects_invalid_stage_name() {
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
            "opencode",
            "--stage-name",
            "Bad_Name",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(contains("OpenCode skill name \"Bad_Name\" is invalid"));
}
