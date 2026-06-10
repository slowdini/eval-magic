//! Codex-harness behavior: `.agents/skills` staging, inline fallback, and the
//! parity-feature rejections.

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
