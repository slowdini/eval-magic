//! Single-skill staging (`stage_skill_for_cc` / `stage_skill_for_harness`) and
//! custom-name registration (`register_staged_skill_for_cleanup`).

use super::super::*;
use super::{read, read_manifest, write};
use std::fs;
use tempfile::TempDir;

// ── stage_skill_for_cc ────────────────────────────────────────────────

#[test]
fn writes_skill_md_and_returns_slug() {
    let tmp = TempDir::new().unwrap();
    let content = "---\nname: example\ndescription: example skill\n---\n\nbody\n";
    let slug = stage_skill_for_cc(&StageSkillOpts {
        content,
        iteration: 3,
        condition: "with_skill",
        skill_name: "verification-before-completion",
        repo_root: tmp.path(),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(
        slug,
        "slow-powers-eval-3-with_skill__verification-before-completion"
    );
    let staged = tmp
        .path()
        .join(".claude/skills")
        .join(&slug)
        .join("SKILL.md");
    assert!(staged.exists());
    assert_eq!(read(&staged), content);
}

#[test]
fn overwrites_existing_staged_skill_at_same_slug() {
    let tmp = TempDir::new().unwrap();
    let opts = |content| StageSkillOpts {
        content,
        iteration: 1,
        condition: "with_skill",
        skill_name: "s",
        repo_root: tmp.path(),
        ..Default::default()
    };
    stage_skill_for_cc(&opts("first")).unwrap();
    let slug = stage_skill_for_cc(&opts("second")).unwrap();
    let staged = tmp
        .path()
        .join(".claude/skills")
        .join(&slug)
        .join("SKILL.md");
    assert_eq!(read(&staged), "second");
}

#[test]
fn copies_sibling_assets_from_assets_dir() {
    let tmp = TempDir::new().unwrap();
    let assets = tmp.path().join("assets-src");
    write(&assets.join("SKILL.md"), "the source skill md");
    write(&assets.join("code-review.md"), "review guidance");
    write(
        &assets.join("scripts").join("helper.ts"),
        "export const x = 1",
    );

    let slug = stage_skill_for_cc(&StageSkillOpts {
        content: "staged content",
        iteration: 1,
        condition: "new_skill",
        skill_name: "s",
        repo_root: tmp.path(),
        assets_dir: Some(&assets),
        ..Default::default()
    })
    .unwrap();

    let staged_dir = tmp.path().join(".claude/skills").join(&slug);
    assert_eq!(read(&staged_dir.join("SKILL.md")), "staged content");
    assert_eq!(read(&staged_dir.join("code-review.md")), "review guidance");
    assert_eq!(
        read(&staged_dir.join("scripts").join("helper.ts")),
        "export const x = 1"
    );
}

#[test]
fn excludes_skill_md_evals_and_snapshot_meta_from_asset_copy() {
    let tmp = TempDir::new().unwrap();
    let assets = tmp.path().join("assets-src");
    write(&assets.join("SKILL.md"), "src skill md");
    write(&assets.join("code-review.md"), "keep me");
    write(&assets.join(SNAPSHOT_META), "{\"source\":\"ref\"}");
    write(&assets.join("evals").join("evals.json"), "{}");

    let slug = stage_skill_for_cc(&StageSkillOpts {
        content: "staged",
        iteration: 1,
        condition: "old_skill",
        skill_name: "s",
        repo_root: tmp.path(),
        assets_dir: Some(&assets),
        ..Default::default()
    })
    .unwrap();

    let staged_dir = tmp.path().join(".claude/skills").join(&slug);
    assert!(staged_dir.join("code-review.md").exists());
    assert!(!staged_dir.join("evals").exists());
    assert!(!staged_dir.join(SNAPSHOT_META).exists());
    assert_eq!(read(&staged_dir.join("SKILL.md")), "staged");
}

#[test]
fn stages_skill_md_alone_when_assets_dir_omitted() {
    let tmp = TempDir::new().unwrap();
    let slug = stage_skill_for_cc(&StageSkillOpts {
        content: "solo",
        iteration: 1,
        condition: "with_skill",
        skill_name: "s",
        repo_root: tmp.path(),
        ..Default::default()
    })
    .unwrap();
    let staged_dir = tmp.path().join(".claude/skills").join(&slug);
    let entries: Vec<_> = fs::read_dir(&staged_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["SKILL.md"]);
}

#[test]
fn stage_name_override_stages_under_verbatim_name() {
    let tmp = TempDir::new().unwrap();
    let content = "---\nname: example\ndescription: example skill\n---\n\nbody\n";
    let slug = stage_skill_for_cc(&StageSkillOpts {
        content,
        iteration: 2,
        condition: "with_skill",
        skill_name: "verification-before-completion",
        repo_root: tmp.path(),
        stage_name_override: Some("verification-before-completion"),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(slug, "verification-before-completion");
    let staged = tmp
        .path()
        .join(".claude/skills")
        .join(&slug)
        .join("SKILL.md");
    assert!(staged.exists());
    assert_eq!(read(&staged), content);
}

// ── stage_skill_for_harness (codex) ───────────────────────────────────

#[test]
fn codex_stages_under_agents_skills_and_rewrites_frontmatter_name() {
    let tmp = TempDir::new().unwrap();
    let content = "---\nname: mr-review\ndescription: review merge requests\n---\n\nbody\n";
    let slug = stage_skill_for_harness(&StageSkillOpts {
        content,
        iteration: 4,
        condition: "with_skill",
        skill_name: "mr-review",
        repo_root: tmp.path(),
        harness: Harness::Codex,
        ..Default::default()
    })
    .unwrap();

    assert_eq!(slug, "slow-powers-eval-4-with_skill__mr-review");
    let staged = tmp
        .path()
        .join(".agents/skills")
        .join(&slug)
        .join("SKILL.md");
    assert!(staged.exists());
    let body = read(&staged);
    assert!(body.contains(&format!("name: {slug}")));
    assert!(body.contains("description: review merge requests"));
    assert!(body.contains("body"));
    assert!(!tmp.path().join(".claude/skills").exists());
}

#[test]
fn codex_stage_name_override_is_dir_and_frontmatter_name() {
    let tmp = TempDir::new().unwrap();
    let slug = stage_skill_for_harness(&StageSkillOpts {
        content: "---\nname: mr-review\ndescription: review merge requests\n---\n\nbody\n",
        iteration: 1,
        condition: "with_skill",
        skill_name: "mr-review",
        repo_root: tmp.path(),
        harness: Harness::Codex,
        stage_name_override: Some("mr-review"),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(slug, "mr-review");
    let staged = read(&tmp.path().join(".agents/skills/mr-review/SKILL.md"));
    assert!(staged.contains("name: mr-review"));
}

// ── register_staged_skill_for_cleanup ─────────────────────────────────

#[test]
fn register_appends_custom_dir_so_cleanup_removes_it() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join(".claude/skills");
    write(
        &skills_dir.join(STAGED_SIBLING_MANIFEST),
        &serde_json::to_string_pretty(&serde_json::json!({
            "created_at": "x",
            "staged_under_test": "verification-before-completion",
            "created_entries": [{ "name": "sibling-a", "preexisting": false }],
        }))
        .unwrap(),
    );
    let custom_dir = skills_dir.join("verification-before-completion");
    write(&custom_dir.join("SKILL.md"), "staged");

    register_staged_skill_for_cleanup(
        tmp.path(),
        "verification-before-completion",
        Harness::ClaudeCode,
    )
    .unwrap();

    let mut names: Vec<String> = read_manifest(&skills_dir)
        .created_entries
        .iter()
        .map(|e| e.name.clone())
        .collect();
    names.sort();
    assert_eq!(names, vec!["sibling-a", "verification-before-completion"]);

    cleanup_staged_skills(tmp.path(), Harness::ClaudeCode).unwrap();
    assert!(!custom_dir.exists());
}

#[test]
fn register_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join(".claude/skills");
    write(
        &skills_dir.join(STAGED_SIBLING_MANIFEST),
        &serde_json::to_string_pretty(&serde_json::json!({
            "created_at": "x",
            "staged_under_test": "foo",
            "created_entries": [],
        }))
        .unwrap(),
    );

    register_staged_skill_for_cleanup(tmp.path(), "foo-staged", Harness::ClaudeCode).unwrap();
    register_staged_skill_for_cleanup(tmp.path(), "foo-staged", Harness::ClaudeCode).unwrap();

    let count = read_manifest(&skills_dir)
        .created_entries
        .iter()
        .filter(|e| e.name == "foo-staged")
        .count();
    assert_eq!(count, 1);
}
