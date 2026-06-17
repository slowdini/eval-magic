//! Workspace lifecycle subcommands: `snapshot`, `promote-baseline`, `teardown`.

use crate::helpers::{canonical_root, skill_eval};
use predicates::str::contains;
use std::fs;

/// Write `<skill-dir>/mr-review/SKILL.md` and return `(skill_dir, skill_sub)`.
fn write_skill_md(root: &std::path::Path, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    fs::create_dir_all(&skill_sub).unwrap();
    fs::write(skill_sub.join("SKILL.md"), body).unwrap();
    (skill_dir, skill_sub)
}

/// Run git with deterministic identity / no signing, asserting success.
fn git(args: &[&str], cwd: &std::path::Path) {
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.email=eval@test",
            "-c",
            "user.name=eval",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("git should run");
    assert!(status.success(), "git {args:?} failed");
}

/// `promote-baseline`: copies benchmark + gradings into the committed baseline,
/// drops the marker, and reports success with an empty stderr.
#[test]
fn promote_baseline_copies_artifacts_and_reports() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill_md(&root, "---\nname: mr-review\n---\nbody\n");

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-2");
    let cond_dir = iteration_dir.join("eval-e1").join("with_skill");
    fs::create_dir_all(&cond_dir).unwrap();
    fs::write(
        iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0.5}}"#,
    )
    .unwrap();
    fs::write(
        cond_dir.join("grading.json"),
        r#"{"summary":{"pass_rate":1}}"#,
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["promote-baseline", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "2"])
        .assert()
        .success()
        .stderr("")
        .stdout(contains("Promoted baseline for mr-review"))
        .stdout(contains("1 grading file "));

    let baseline = skill_sub.join("evals").join("baseline");
    assert!(baseline.join("benchmark.json").exists());
    assert!(baseline.join("grading/e1__with_skill.json").exists());
    assert!(baseline.join("BASELINE.md").exists());
    assert!(iteration_dir.join(".promoted.json").exists());
}

/// `promote-baseline`: a fresh promotion (no prior NOTES.md) writes a stub and
/// says so on stdout, keeping stderr clean.
#[test]
fn promote_baseline_writes_notes_stub_and_reports_it() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill_md(&root, "---\nname: mr-review\n---\nbody\n");

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    fs::create_dir_all(&iteration_dir).unwrap();
    fs::write(
        iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["promote-baseline", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .success()
        .stderr("")
        .stdout(contains("NOTES.md stub"));

    let notes =
        fs::read_to_string(skill_sub.join("evals").join("baseline").join("NOTES.md")).unwrap();
    assert!(notes.contains("iteration-1"));
}

/// `promote-baseline`: a pre-existing NOTES.md is kept byte-identical, and a
/// warning on stderr says it was retained from the prior baseline.
#[test]
fn promote_baseline_warns_when_prior_notes_retained() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill_md(&root, "---\nname: mr-review\n---\nbody\n");

    let baseline = skill_sub.join("evals").join("baseline");
    fs::create_dir_all(&baseline).unwrap();
    fs::write(baseline.join("NOTES.md"), "notes from iteration-1\n").unwrap();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-2");
    fs::create_dir_all(&iteration_dir).unwrap();
    fs::write(
        iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["promote-baseline", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "2"])
        .assert()
        .success()
        .stderr(contains("NOTES.md retained from prior baseline"));

    assert_eq!(
        fs::read_to_string(baseline.join("NOTES.md")).unwrap(),
        "notes from iteration-1\n"
    );
}

/// `promote-baseline`: a missing iteration fails non-zero, naming the iteration.
#[test]
fn promote_baseline_fails_clearly_when_iteration_missing() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, _skill_sub) = write_skill_md(&root, "---\nname: mr-review\n---\nbody\n");
    let cwd = root.join("work");
    fs::create_dir_all(&cwd).unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["promote-baseline", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "9"])
        .assert()
        .failure()
        .stderr(contains("iteration-9"));
}

/// `snapshot` (working tree): copies SKILL.md, records working-tree provenance.
#[test]
fn snapshot_working_tree_copies_and_records_provenance() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, _skill_sub) = write_skill_md(&root, "v2 working tree\n");
    let cwd = root.join("work");
    fs::create_dir_all(&cwd).unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["snapshot", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--label", "wt"])
        .assert()
        .success()
        .stdout(contains("Snapshotted mr-review →"));

    let snap = cwd.join("skills-workspace/mr-review/snapshots/wt");
    assert_eq!(
        fs::read_to_string(snap.join("SKILL.md")).unwrap(),
        "v2 working tree\n"
    );
    let meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(snap.join(".snapshot-meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["source"], "working-tree");
}

#[test]
fn snapshot_defaults_to_baseline_label() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, _skill_sub) = write_skill_md(&root, "v2 working tree\n");
    let cwd = root.join("work");
    fs::create_dir_all(&cwd).unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["snapshot", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success()
        .stdout(contains("Snapshotted mr-review"));

    assert_eq!(
        fs::read_to_string(cwd.join("skills-workspace/mr-review/snapshots/baseline/SKILL.md"))
            .unwrap(),
        "v2 working tree\n"
    );
}

/// `snapshot --ref`: reads the committed content from a git ref, leaving the
/// working tree untouched, and records ref provenance.
#[test]
fn snapshot_ref_reads_committed_content() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill_md(&root, "v1 baseline\n");
    git(&["init", "-q"], &root);
    git(&["add", "-A"], &root);
    git(&["commit", "-q", "-m", "v1"], &root);
    fs::write(skill_sub.join("SKILL.md"), "v2 working tree\n").unwrap();

    let cwd = root.join("work");
    fs::create_dir_all(&cwd).unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["snapshot", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--label", "old", "--ref", "HEAD"])
        .assert()
        .success()
        .stdout(contains("Snapshotted mr-review at HEAD →"));

    let snap = cwd.join("skills-workspace/mr-review/snapshots/old");
    assert_eq!(
        fs::read_to_string(snap.join("SKILL.md")).unwrap(),
        "v1 baseline\n"
    );
    // Working tree still holds v2 (no clobber).
    assert_eq!(
        fs::read_to_string(skill_sub.join("SKILL.md")).unwrap(),
        "v2 working tree\n"
    );
    let meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(snap.join(".snapshot-meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["source"], "ref");
    assert_eq!(meta["ref"], "HEAD");
}

/// `teardown`: removes a promoted iteration, keeps an unpromoted one with
/// results, and warns about it (pointing at `promote-baseline`).
#[test]
fn teardown_reclaims_promoted_and_keeps_uncommitted() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, _skill_sub) = write_skill_md(&root, "---\nname: mr-review\n---\nbody\n");

    let cwd = root.join("work");
    let skill_ws = cwd.join("skills-workspace").join("mr-review");
    let promoted = skill_ws.join("iteration-1");
    let kept = skill_ws.join("iteration-2");
    fs::create_dir_all(&promoted).unwrap();
    fs::create_dir_all(&kept).unwrap();
    fs::write(promoted.join("benchmark.json"), "{}").unwrap();
    fs::write(promoted.join(".promoted.json"), r#"{"commit":"abc"}"#).unwrap();
    fs::write(kept.join("benchmark.json"), "{}").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["teardown", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success()
        .stdout(contains("Reclaimed 1 workspace iteration(s)"))
        .stderr(contains("Kept 1 workspace iteration(s)"))
        .stderr(contains("promote-baseline"));

    assert!(!promoted.exists());
    assert!(kept.exists());
}
