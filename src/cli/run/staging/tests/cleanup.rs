//! Teardown (`cleanup_staged_skills`): prefix scan, manifest-aware restore, and
//! the runner-created-tree removal/prune cases.

use super::super::*;
use super::{read, write};
use std::fs;
use tempfile::TempDir;

// ── basic ─────────────────────────────────────────────────────────────

#[test]
fn cleanup_removes_only_prefixed_dirs() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join(".claude/skills");
    let staged_a = skills_dir.join("slow-powers-eval-1-with_skill__foo");
    let staged_b = skills_dir.join("slow-powers-eval-1-new_skill__bar");
    let production = skills_dir.join("user-custom-skill");
    for d in [&staged_a, &staged_b, &production] {
        fs::create_dir_all(d).unwrap();
    }

    cleanup_staged_skills(tmp.path(), Harness::resolve("claude-code").unwrap()).unwrap();

    assert!(!staged_a.exists());
    assert!(!staged_b.exists());
    assert!(production.exists());
}

#[test]
fn cleanup_is_noop_when_skills_dir_missing() {
    let tmp = TempDir::new().unwrap();
    cleanup_staged_skills(tmp.path(), Harness::resolve("claude-code").unwrap()).unwrap();
}

// ── manifest-aware ────────────────────────────────────────────────────

#[test]
fn codex_cleanup_restores_preexisting_agents_entries() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src-skills");
    write(&src.join("alpha/SKILL.md"), "new alpha");
    let skills_dir = tmp.path().join(".agents/skills");
    write(&skills_dir.join("alpha/SKILL.md"), "USER ALPHA");

    stage_sibling_skills(&StageSiblingOpts {
        skill_under_test: "x",
        skills_source_dir: &src,
        repo_root: tmp.path(),
        harness: Harness::resolve("codex").unwrap(),
    })
    .unwrap();
    assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "new alpha");

    cleanup_staged_skills(tmp.path(), Harness::resolve("codex").unwrap()).unwrap();

    assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "USER ALPHA");
    assert!(!skills_dir.join(STAGED_SIBLING_MANIFEST).exists());
    assert!(!tmp.path().join(".claude/skills").exists());
}

#[test]
fn removes_sibling_entries_and_restores_backups() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src-skills");
    write(&src.join("alpha/SKILL.md"), "new alpha");
    write(&src.join("beta/SKILL.md"), "new beta");
    let skills_dir = tmp.path().join(".claude/skills");
    write(&skills_dir.join("alpha/SKILL.md"), "USER ALPHA");

    stage_sibling_skills(&StageSiblingOpts {
        skill_under_test: "x",
        skills_source_dir: &src,
        repo_root: tmp.path(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "new alpha");
    assert_eq!(read(&skills_dir.join("beta/SKILL.md")), "new beta");

    cleanup_staged_skills(tmp.path(), Harness::resolve("claude-code").unwrap()).unwrap();

    assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "USER ALPHA");
    assert!(!skills_dir.join("beta").exists());
    assert!(!skills_dir.join(STAGED_SIBLING_MANIFEST).exists());
}

#[test]
fn sweeps_prefix_entries_when_no_manifest() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join(".claude/skills");
    fs::create_dir_all(skills_dir.join("slow-powers-eval-1-with_skill__foo")).unwrap();
    fs::create_dir_all(skills_dir.join("user-custom")).unwrap();

    cleanup_staged_skills(tmp.path(), Harness::resolve("claude-code").unwrap()).unwrap();

    assert!(
        !skills_dir
            .join("slow-powers-eval-1-with_skill__foo")
            .exists()
    );
    assert!(skills_dir.join("user-custom").exists());
}

// ── runner-created .claude/skills ─────────────────────────────────────

#[test]
fn removes_whole_tree_when_runner_created_and_prunes_claude() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src-skills");
    write(&src.join("alpha/SKILL.md"), "alpha");

    stage_sibling_skills(&StageSiblingOpts {
        skill_under_test: "x",
        skills_source_dir: &src,
        repo_root: tmp.path(),
        ..Default::default()
    })
    .unwrap();
    fs::create_dir_all(tmp.path().join(".claude/skills/stray-leftover")).unwrap();

    cleanup_staged_skills(tmp.path(), Harness::resolve("claude-code").unwrap()).unwrap();

    assert!(!tmp.path().join(".claude/skills").exists());
    assert!(!tmp.path().join(".claude").exists());
}

#[test]
fn keeps_claude_and_settings_when_runner_created_only_skills() {
    let tmp = TempDir::new().unwrap();
    let claude_dir = tmp.path().join(".claude");
    write(&claude_dir.join("settings.json"), "{}");
    let src = tmp.path().join("src-skills");
    write(&src.join("alpha/SKILL.md"), "alpha");

    stage_sibling_skills(&StageSiblingOpts {
        skill_under_test: "x",
        skills_source_dir: &src,
        repo_root: tmp.path(),
        ..Default::default()
    })
    .unwrap();

    cleanup_staged_skills(tmp.path(), Harness::resolve("claude-code").unwrap()).unwrap();

    assert!(!claude_dir.join("skills").exists());
    assert!(claude_dir.exists());
    assert!(claude_dir.join("settings.json").exists());
}

#[test]
fn leaves_preexisting_skills_dir_in_place() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join(".claude/skills");
    write(&skills_dir.join("user-owned/SKILL.md"), "USER");
    let src = tmp.path().join("src-skills");
    write(&src.join("alpha/SKILL.md"), "alpha");

    stage_sibling_skills(&StageSiblingOpts {
        skill_under_test: "x",
        skills_source_dir: &src,
        repo_root: tmp.path(),
        ..Default::default()
    })
    .unwrap();

    cleanup_staged_skills(tmp.path(), Harness::resolve("claude-code").unwrap()).unwrap();

    assert!(skills_dir.exists());
    assert_eq!(read(&skills_dir.join("user-owned/SKILL.md")), "USER");
    assert!(!skills_dir.join("alpha").exists());
}

#[test]
fn codebase_skill_exclusion_moves_all_discovery_roots_and_cleanup_restores_them() {
    let tmp = TempDir::new().unwrap();
    write(
        &tmp.path().join(".opencode/skills/native/SKILL.md"),
        "NATIVE",
    );
    write(
        &tmp.path().join(".claude/skills/claude-compat/SKILL.md"),
        "CLAUDE",
    );
    write(
        &tmp.path().join(".agents/skills/agents-compat/SKILL.md"),
        "AGENTS",
    );
    write(&tmp.path().join(".opencode/settings.json"), "{}");
    write(&tmp.path().join("CLAUDE.md"), "claude instructions");
    write(&tmp.path().join("AGENTS.md"), "agent instructions");

    exclude_codebase_skill_sources(tmp.path(), "subject", Harness::resolve("opencode").unwrap())
        .unwrap();

    assert!(!tmp.path().join(".opencode/skills/native").exists());
    assert!(!tmp.path().join(".claude/skills").exists());
    assert!(!tmp.path().join(".agents/skills").exists());
    assert_eq!(read(&tmp.path().join(".opencode/settings.json")), "{}");
    assert_eq!(read(&tmp.path().join("CLAUDE.md")), "claude instructions");
    assert_eq!(read(&tmp.path().join("AGENTS.md")), "agent instructions");

    cleanup_staged_skills(tmp.path(), Harness::resolve("opencode").unwrap()).unwrap();

    assert_eq!(
        read(&tmp.path().join(".opencode/skills/native/SKILL.md")),
        "NATIVE"
    );
    assert_eq!(
        read(&tmp.path().join(".claude/skills/claude-compat/SKILL.md")),
        "CLAUDE"
    );
    assert_eq!(
        read(&tmp.path().join(".agents/skills/agents-compat/SKILL.md")),
        "AGENTS"
    );
}

#[test]
fn cleanup_rejects_undeclared_excluded_root_before_removing_native_skills() {
    let tmp = TempDir::new().unwrap();
    let harness = Harness::resolve("opencode").unwrap();
    write(
        &tmp.path().join(".opencode/skills/native/SKILL.md"),
        "NATIVE",
    );
    let backup = make_backup_root().unwrap().join("skill-root");
    write(&backup.join("subject/SKILL.md"), "SUBJECT");
    let manifest = SiblingManifest {
        created_at: "test".into(),
        staged_under_test: "subject".into(),
        skills_dir_preexisting: Some(true),
        created_entries: Vec::new(),
        excluded_roots: vec![ExcludedRoot {
            path: "undeclared/skills".into(),
            backup_path: backup.display().to_string(),
        }],
    };
    write_json(
        &tmp.path()
            .join(".opencode/skills")
            .join(STAGED_SIBLING_MANIFEST),
        &manifest,
    )
    .unwrap();

    let error = cleanup_staged_skills(tmp.path(), harness).unwrap_err();

    assert!(error.to_string().contains("undeclared project skill root"));
    assert_eq!(
        read(&tmp.path().join(".opencode/skills/native/SKILL.md")),
        "NATIVE"
    );
}

#[test]
fn cleanup_rejects_unmanaged_exclusion_backup_before_removing_native_skills() {
    let tmp = TempDir::new().unwrap();
    let harness = Harness::resolve("opencode").unwrap();
    write(
        &tmp.path().join(".opencode/skills/native/SKILL.md"),
        "NATIVE",
    );
    let backup = tmp.path().join("unmanaged/skill-root");
    write(&backup.join("subject/SKILL.md"), "SUBJECT");
    let manifest = SiblingManifest {
        created_at: "test".into(),
        staged_under_test: "subject".into(),
        skills_dir_preexisting: Some(true),
        created_entries: Vec::new(),
        excluded_roots: vec![ExcludedRoot {
            path: ".opencode/skills".into(),
            backup_path: backup.display().to_string(),
        }],
    };
    write_json(
        &tmp.path()
            .join(".opencode/skills")
            .join(STAGED_SIBLING_MANIFEST),
        &manifest,
    )
    .unwrap();

    let error = cleanup_staged_skills(tmp.path(), harness).unwrap_err();

    assert!(error.to_string().contains("unmanaged exclusion backup"));
    assert_eq!(
        read(&tmp.path().join(".opencode/skills/native/SKILL.md")),
        "NATIVE"
    );
    assert!(backup.exists());
}
