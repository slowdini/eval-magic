//! The skill under test as a sourced, copied input.
//!
//! Asserted across the run boundary rather than in unit tests because the
//! property spans resolution, the copy, staging, and every provenance artifact —
//! the same reason `codebase.rs` sits here.

use crate::helpers::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Prepare an iteration from `skill_dir` and return its directory.
fn prepare(cwd: &Path, skill_dir: &Path, extra: &[&str]) -> std::path::PathBuf {
    let mut cmd = skill_eval();
    cmd.current_dir(cwd)
        .args(["run", "--skill-dir"])
        .arg(skill_dir)
        .args(["--skill", "mr-review", "--dry-run"])
        .args(extra);
    cmd.assert().success();
    iteration_dir(cwd)
}

/// The isolation claim in one assertion: what a condition stages is a copy the
/// runner placed in the eval home, never the operator's own tree.
#[test]
fn a_condition_stages_the_copy_in_the_eval_home_not_the_live_tree() {
    let tmp = tempfile::TempDir::new().unwrap();
    // realpath: this test compares paths the CLI emits, and the CLI resolves
    // its roots once, so the expectation has to be built from a resolved root.
    let (skill_dir, cwd) = setup(&resolved(tmp.path()), DEFAULT_EVALS);

    let iteration = prepare(&cwd, &skill_dir, &["--mode", "new-skill"]);

    let copy = iteration.join(".skills").join("mr-review").join("SKILL.md");
    assert!(
        copy.exists(),
        "the skill was not copied into {}",
        iteration.join(".skills").display()
    );

    let conditions = read_json(&iteration.join("conditions.json"));
    let staged_from = conditions["conditions"][0]["skill_path"]
        .as_str()
        .expect("the staging arm names a skill path");
    assert_eq!(staged_from, wire_path(&copy));
    assert!(
        !staged_from.contains("skill-dir"),
        "still staging from the live tree: {staged_from}"
    );
}

/// The copy is taken from the working tree, not from a commit. Mode B's new arm
/// *is* the uncommitted edit under test, and Mode A's ordinary loop is
/// edit-then-run; a committed-state copy would measure the wrong bytes.
#[test]
fn the_copy_carries_an_uncommitted_edit() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    fs::write(
        skill_dir.join("mr-review").join("SKILL.md"),
        "---\nname: mr-review\ndescription: review merge requests\n---\n\nEDITED BUT NEVER COMMITTED\n",
    )
    .unwrap();

    let iteration = prepare(&cwd, &skill_dir, &["--mode", "new-skill"]);

    let copied = read_str(&iteration.join(".skills").join("mr-review").join("SKILL.md"));
    assert!(
        copied.contains("EDITED BUT NEVER COMMITTED"),
        "copied: {copied}"
    );
}

/// `evals/` is the eval author's material, not the agent's. It rides into the
/// copy because fixtures are read from there, and must still be filtered out of
/// what the agent can discover.
#[test]
fn the_copy_keeps_evals_while_the_staged_skill_still_excludes_them() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    let iteration = prepare(&cwd, &skill_dir, &["--mode", "new-skill"]);

    assert!(
        iteration
            .join(".skills")
            .join("mr-review")
            .join("evals")
            .join("evals.json")
            .exists(),
        "the copy dropped evals/, which fixtures are read from"
    );
    let staged = cli_env_dir(&cwd, "g1", "with_skill")
        .join(".claude/skills")
        .join("slow-powers-eval-1-with_skill__mr-review");
    assert!(staged.join("SKILL.md").exists(), "skill was not staged");
    assert!(
        !staged.join("evals").exists(),
        "the staged skill exposes the eval definitions to the agent"
    );
}

/// Provenance: the resolved skill source reaches `conditions.json`, so a report
/// can name the tree it measured on the skill side as well as the codebase side.
#[test]
fn the_resolved_skill_source_reaches_conditions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    let iteration = prepare(&cwd, &skill_dir, &["--mode", "new-skill"]);

    let conditions = read_json(&iteration.join("conditions.json"));
    let source = &conditions["skill_source"];
    assert_eq!(source["kind"], Value::from("path"));
    assert_eq!(
        source["resolved_path"],
        Value::from(wire_path(&resolved(&skill_dir.join("mr-review"))))
    );
    assert_eq!(
        source["host_local"],
        Value::from(true),
        "a skill named by path is not resolvable off this host"
    );
}

/// The roster is captured once, at resolution, rather than rescanned from the
/// live tree while each environment is staged.
#[test]
fn the_sibling_roster_is_recorded_and_each_sibling_is_copied_once() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let helper = skill_dir.join("helper-skill");
    fs::create_dir_all(&helper).unwrap();
    fs::write(
        helper.join("SKILL.md"),
        "---\nname: helper-skill\ndescription: helper\n---\n\nhelper\n",
    )
    .unwrap();

    let iteration = prepare(&cwd, &skill_dir, &["--mode", "new-skill"]);

    let conditions = read_json(&iteration.join("conditions.json"));
    assert_eq!(
        conditions["skill_source"]["siblings"],
        Value::from(vec!["helper-skill"])
    );
    assert!(
        iteration
            .join(".skills")
            .join("helper-skill")
            .join("SKILL.md")
            .exists(),
        "the sibling was not copied into the eval home"
    );
}

/// `--workspace-dir` may legitimately land inside the skill tree — pointing it at
/// `.eval-magic` from inside a skill is the obvious way to keep artifacts next to
/// the work. The copy must not then contain itself.
#[test]
fn a_workspace_inside_the_skill_tree_is_not_copied_into_itself() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, _cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let skill_sub = skill_dir.join("mr-review");

    skill_eval()
        .current_dir(&skill_sub)
        .args(["run", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let copy = skill_sub
        .join(".eval-magic")
        .join("mr-review")
        .join("iteration-1")
        .join(".skills")
        .join("mr-review");
    assert!(copy.join("SKILL.md").exists(), "the skill was not copied");
    assert!(
        !copy.join(".eval-magic").exists(),
        "the copy swallowed the workspace it lives in"
    );
}

/// The copy carries uncommitted work, so the warning has to say the run is
/// *measuring* it — the opposite of the codebase warning, where a clean checkout
/// leaves it behind. One sentence for both subjects would be wrong for one.
#[test]
fn an_uncommitted_skill_warns_that_the_run_measures_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let skill_sub = skill_dir.join("mr-review");
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    // A repository, so there is a revision the uncommitted edit departs from.
    for args in [
        vec!["init", "--quiet", "--initial-branch", "main", "."],
        vec!["add", "--all"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(&skill_dir)
            .status()
            .unwrap();
    }
    std::process::Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@localhost",
            "commit",
            "--quiet",
            "--no-gpg-sign",
            "-m",
            "initial",
        ])
        .current_dir(&skill_dir)
        .status()
        .unwrap();
    fs::write(
        skill_sub.join("SKILL.md"),
        "---\nname: mr-review\ndescription: d\n---\n\nedited\n",
    )
    .unwrap();

    let output = skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        stderr.contains("uncommitted"),
        "no uncommitted-work warning; stderr was: {stderr}"
    );
    assert!(
        !stderr.contains("does not include them"),
        "the skill was told its edit was dropped, but the copy carries it: {stderr}"
    );

    let iteration = iteration_dir(&cwd);
    let conditions = read_json(&iteration.join("conditions.json"));
    assert_eq!(conditions["skill_source"]["dirty"], Value::from(true));
}

/// Grading reads the iteration's own copy, not the live tree. Editing the eval
/// definitions between `run` and `grade` would otherwise silently change what a
/// finished run is measured against — the provenance hole this ticket closes,
/// one phase later.
///
/// The live copy is made *unreadable* rather than merely different: that is the
/// difference an assertion can see, since `grade` does not echo eval ids.
#[test]
fn grading_reads_the_eval_definitions_the_run_copied() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    let iteration = prepare(&cwd, &skill_dir, &["--mode", "new-skill"]);

    fs::write(
        skill_dir.join("mr-review").join("evals").join("evals.json"),
        "{ not valid json at all",
    )
    .unwrap();

    let copied: Value = read_json(
        &iteration
            .join(".skills")
            .join("mr-review")
            .join("evals")
            .join("evals.json"),
    );
    assert_eq!(
        copied["evals"][0]["id"], "e1",
        "the copy should still hold what the run was built from"
    );

    skill_eval()
        .current_dir(&cwd)
        .args(["grade", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .success();
}

/// Mode B parity. The `old_skill` arm stages a snapshot the workspace already
/// held; the `new_skill` arm must stage the copy, so both arms are things the
/// runner placed and neither is read from the operator's tree.
#[test]
fn revision_mode_stages_the_snapshot_and_the_copy() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(&resolved(tmp.path()), DEFAULT_EVALS);

    skill_eval()
        .current_dir(&cwd)
        .args(["snapshot", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--label", "baseline"])
        .assert()
        .success();

    let iteration = prepare(&cwd, &skill_dir, &["--mode", "revision"]);

    let conditions = read_json(&iteration.join("conditions.json"));
    let arms = conditions["conditions"].as_array().unwrap();
    assert_eq!(arms[0]["name"], "old_skill");
    assert_eq!(arms[1]["name"], "new_skill");

    let old_arm = arms[0]["skill_path"].as_str().unwrap();
    assert!(
        old_arm.contains("/snapshots/baseline/"),
        "old arm should stage the snapshot, was {old_arm}"
    );

    let new_arm = arms[1]["skill_path"].as_str().unwrap();
    assert_eq!(
        new_arm,
        wire_path(&iteration.join(".skills").join("mr-review").join("SKILL.md"))
    );

    // Both arms name something the runner placed inside the eval home.
    for arm in [old_arm, new_arm] {
        assert!(
            arm.starts_with(&wire_path(&cwd.join(".eval-magic"))),
            "arm reads from outside the eval home: {arm}"
        );
    }

    assert_eq!(
        conditions["skill_source"]["kind"],
        Value::from("path"),
        "revision mode records the skill source too"
    );
}

/// `--no-stage` puts nothing in the harness skills directory, so recording a
/// roster of siblings "staged alongside" the skill would describe an environment
/// that never existed.
#[test]
fn no_stage_records_no_sibling_roster() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let helper = skill_dir.join("helper-skill");
    fs::create_dir_all(&helper).unwrap();
    fs::write(
        helper.join("SKILL.md"),
        "---\nname: helper-skill\ndescription: helper\n---\n\nhelper\n",
    )
    .unwrap();

    let iteration = prepare(&cwd, &skill_dir, &["--mode", "new-skill", "--no-stage"]);

    let conditions = read_json(&iteration.join("conditions.json"));
    assert!(
        conditions["skill_source"]["siblings"].is_null(),
        "recorded a roster nothing staged: {}",
        conditions["skill_source"]
    );
}
