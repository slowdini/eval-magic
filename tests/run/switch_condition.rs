//! `switch-condition`: the per-condition read-isolation barrier for a
//! single-session isolated run. It removes the off-condition's staged skill from
//! `env/.claude/skills/` between dispatch batches, and must resolve the iteration
//! tree while invoked from `cwd = env/`.

use crate::helpers::*;
use std::path::{Path, PathBuf};

const WITH_SLUG: &str = "slow-powers-eval-1-with_skill__mr-review";

fn env_skills_dir(cwd: &Path) -> PathBuf {
    env_dir(cwd).join(".claude/skills")
}

/// Run `switch-condition` the way the runbook prescribes: from inside `env/`,
/// carrying only the self-sufficient `--skill-dir/--skill/--workspace-dir` selector.
fn switch_to(cwd: &Path, skill_dir: &Path, condition: &str) -> assert_cmd::assert::Assert {
    skill_eval()
        .current_dir(env_dir(cwd))
        .args(["switch-condition", "--skill-dir"])
        .arg(skill_dir)
        .args(["--skill", "mr-review", "--workspace-dir"])
        .arg(cwd.join("skills-workspace"))
        .args(["--iteration", "1", "--condition", condition])
        .assert()
}

#[test]
fn switch_condition_removes_off_condition_slug_from_env_cwd() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // Build the env (staging happens even under --dry-run).
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let with_slug = env_skills_dir(&cwd).join(WITH_SLUG);
    assert!(with_slug.is_dir(), "with_skill staged before switch");

    // Move to the without_skill batch: the off-condition (with_skill) staged skill
    // is removed so the control arm cannot read it.
    switch_to(&cwd, &skill_dir, "without_skill").success();

    assert!(!with_slug.exists(), "with_skill slug removed after switch");
}

#[test]
fn switch_condition_is_idempotent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    // Two switches in a row: the second is a no-op, not an error (a re-run after a
    // fix, or an over-eager operator, must stay safe).
    switch_to(&cwd, &skill_dir, "without_skill").success();
    switch_to(&cwd, &skill_dir, "without_skill").success();
    assert!(!env_skills_dir(&cwd).join(WITH_SLUG).exists());
}

#[test]
fn switch_condition_preserves_guard_marker() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // A guarded run arms the write guard; --guard requires a real (non-dry) run.
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();

    // The guard marker is a sibling file of the slug subtree inside the skills dir.
    let marker = env_skills_dir(&cwd).join(".slow-powers-eval-guard.json");
    assert!(marker.exists(), "guard armed before switch");

    switch_to(&cwd, &skill_dir, "without_skill").success();

    assert!(
        !env_skills_dir(&cwd).join(WITH_SLUG).exists(),
        "slug removed"
    );
    assert!(marker.exists(), "guard marker survives the switch");
}

#[test]
fn switch_condition_rejects_unknown_condition() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    switch_to(&cwd, &skill_dir, "bogus_condition")
        .failure()
        .stderr(predicates::str::contains(
            "unknown --condition 'bogus_condition'",
        ));
    // A typo must not silently leave the staged skill in place under a false sense
    // of isolation.
    assert!(env_skills_dir(&cwd).join(WITH_SLUG).is_dir());
}

#[test]
fn switch_condition_revision_removes_old_skill_keeps_new() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // Revision mode compares a baseline snapshot (old_skill) against the working
    // SKILL.md (new_skill); both arms stage a skill. Seed the baseline snapshot.
    let snapshot = iteration_dir(&cwd)
        .parent()
        .unwrap()
        .join("snapshots")
        .join("baseline");
    std::fs::create_dir_all(&snapshot).unwrap();
    std::fs::write(
        snapshot.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review merge requests\n---\n\nold body\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "revision", "--dry-run"])
        .assert()
        .success();

    let old_slug = env_skills_dir(&cwd).join("slow-powers-eval-1-old_skill__mr-review");
    let new_slug = env_skills_dir(&cwd).join("slow-powers-eval-1-new_skill__mr-review");
    assert!(old_slug.is_dir() && new_slug.is_dir(), "both arms staged");

    // Switch to the new_skill batch: only the old_skill slug is removed.
    switch_to(&cwd, &skill_dir, "new_skill").success();

    assert!(!old_slug.exists(), "old_skill slug removed");
    assert!(new_slug.is_dir(), "new_skill slug kept");
}
