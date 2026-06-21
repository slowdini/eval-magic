//! `reset-batch`: the per-group isolation barrier for a single-session (in-session)
//! isolated run. Between eval-group batches it wipes the shared `env/` working tree
//! — keeping the staged skills and the outputs tree — and re-seeds it with the next
//! group's fixtures, so a prior batch's fixtures and stray writes can't leak.

use crate::helpers::*;
use predicates::str::contains;
use std::fs;
use std::path::Path;

const WITH_SLUG: &str = "slow-powers-eval-1-with_skill__mr-review";

/// Two evals routed into two groups: e2's `isolation: isolated` hint forces its own
/// group, so the in-session env stages group g1 (e1/a.txt) up front and swaps in
/// group g2 (e2/b.txt) via reset-batch.
const TWO_GROUPS: &str = r#"{ "skill_name": "mr-review", "evals": [
    { "id": "e1", "prompt": "p1", "expected_output": "o", "files": ["a.txt"] },
    { "id": "e2", "prompt": "p2", "expected_output": "o", "files": ["b.txt"], "isolation": "isolated" } ] }"#;

/// Stage a two-group interactive iteration; returns `(skill_dir, cwd)` with `env/`
/// holding group g1's fixtures.
fn setup_two_groups(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let (skill_dir, cwd) = setup(root, TWO_GROUPS);
    fs::write(skill_dir.join("mr-review/evals/a.txt"), "AAA").unwrap();
    fs::write(skill_dir.join("mr-review/evals/b.txt"), "BBB").unwrap();
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();
    (skill_dir, cwd)
}

/// Run `reset-batch` the way the runbook prescribes: from inside `env/`, carrying
/// only the self-sufficient `--skill-dir/--skill/--workspace-dir` selector.
fn reset_to(cwd: &Path, skill_dir: &Path, group: &str) -> assert_cmd::assert::Assert {
    skill_eval()
        .current_dir(env_dir(cwd))
        .args(["reset-batch", "--skill-dir"])
        .arg(skill_dir)
        .args(["--skill", "mr-review", "--workspace-dir"])
        .arg(cwd.join("skills-workspace"))
        .args(["--iteration", "1", "--group", group])
        .assert()
}

#[test]
fn reset_batch_wipes_working_tree_and_reseeds_group_fixtures() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup_two_groups(tmp.path());

    // Up front the env holds group g1's fixture only.
    assert_eq!(read_str(&env_dir(&cwd).join("a.txt")), "AAA");
    assert!(!env_dir(&cwd).join("b.txt").exists());
    // Simulate a stray file the g1 batch's agent wrote into the env.
    fs::write(env_dir(&cwd).join("stray.txt"), "STRAY").unwrap();

    reset_to(&cwd, &skill_dir, "g2").success();

    // The env is now seeded for g2: its fixture present, g1's gone, the stray write
    // gone — a clean tree.
    assert_eq!(read_str(&env_dir(&cwd).join("b.txt")), "BBB");
    assert!(!env_dir(&cwd).join("a.txt").exists());
    assert!(!env_dir(&cwd).join("stray.txt").exists());

    // The staged skill and the outputs tree survive the wipe.
    assert!(
        env_dir(&cwd)
            .join(".claude/skills")
            .join(WITH_SLUG)
            .is_dir(),
        "the staged skill survives reset-batch"
    );
    assert!(env_dir(&cwd).join(".eval-magic/outputs").exists());
}

#[test]
fn reset_batch_can_restore_the_first_group() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup_two_groups(tmp.path());

    // Move to g2, then back to g1 (as condition B's loop does after condition A left
    // the env on the last group).
    reset_to(&cwd, &skill_dir, "g2").success();
    reset_to(&cwd, &skill_dir, "g1").success();
    assert_eq!(read_str(&env_dir(&cwd).join("a.txt")), "AAA");
    assert!(!env_dir(&cwd).join("b.txt").exists());
}

#[test]
fn reset_batch_rejects_unknown_group() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup_two_groups(tmp.path());
    reset_to(&cwd, &skill_dir, "g99")
        .failure()
        .stderr(contains("unknown --group"));
}

#[test]
fn reset_batch_on_single_group_run_explains_it_is_unneeded() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    // A single-group run tags no task with a group, so reset-batch has nothing to do
    // and says so rather than silently wiping.
    reset_to(&cwd, &skill_dir, "g1")
        .failure()
        .stderr(contains("single group"));
}
