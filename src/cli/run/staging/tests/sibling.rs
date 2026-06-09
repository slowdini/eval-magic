//! Sibling-skill staging (`stage_sibling_skills`): copy each non-test sibling
//! minus its `evals/`, backing up colliding pre-existing entries.

use super::super::*;
use super::{build_source_skills, read, read_manifest, write};
use std::path::Path;
use tempfile::TempDir;

#[test]
fn stages_each_sibling_minus_evals() {
    let tmp = TempDir::new().unwrap();
    let src = build_source_skills(tmp.path());
    stage_sibling_skills(&StageSiblingOpts {
        skill_under_test: "gamma",
        skills_source_dir: &src,
        repo_root: tmp.path(),
        ..Default::default()
    })
    .unwrap();

    let skills_dir = tmp.path().join(".claude/skills");
    assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "alpha content");
    assert_eq!(read(&skills_dir.join("alpha/helper.md")), "alpha helper");
    assert!(!skills_dir.join("alpha/evals").exists());
    assert_eq!(read(&skills_dir.join("beta/SKILL.md")), "beta content");
    assert!(!skills_dir.join("gamma").exists());

    let mut names: Vec<String> = read_manifest(&skills_dir)
        .created_entries
        .iter()
        .map(|e| e.name.clone())
        .collect();
    names.sort();
    assert_eq!(names, vec!["alpha", "beta"]);
    assert!(
        read_manifest(&skills_dir)
            .created_entries
            .iter()
            .all(|e| !e.preexisting)
    );
}

#[test]
fn backs_up_colliding_preexisting_entries() {
    let tmp = TempDir::new().unwrap();
    let src = build_source_skills(tmp.path());
    let skills_dir = tmp.path().join(".claude/skills");
    write(&skills_dir.join("alpha/SKILL.md"), "USER OWNED");

    stage_sibling_skills(&StageSiblingOpts {
        skill_under_test: "gamma",
        skills_source_dir: &src,
        repo_root: tmp.path(),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "alpha content");
    let manifest = read_manifest(&skills_dir);
    let alpha = manifest
        .created_entries
        .iter()
        .find(|e| e.name == "alpha")
        .unwrap();
    assert!(alpha.preexisting);
    let backup = alpha.backup_path.as_deref().unwrap();
    assert!(Path::new(backup).exists());
    assert_eq!(read(&Path::new(backup).join("SKILL.md")), "USER OWNED");
}

#[test]
fn skips_skill_under_test_in_source() {
    let tmp = TempDir::new().unwrap();
    let src = build_source_skills(tmp.path());
    stage_sibling_skills(&StageSiblingOpts {
        skill_under_test: "alpha",
        skills_source_dir: &src,
        repo_root: tmp.path(),
        ..Default::default()
    })
    .unwrap();
    let skills_dir = tmp.path().join(".claude/skills");
    assert!(!skills_dir.join("alpha").exists());
    assert!(skills_dir.join("beta").exists());
    assert!(skills_dir.join("gamma").exists());
}
