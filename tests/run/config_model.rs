//! Breaking eval-configuration contracts for the single codebase-backed model.

use crate::helpers::*;
use predicates::str::contains;

#[test]
fn missing_effective_codebase_fails_before_creating_an_iteration() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup_raw(tmp.path(), DEFAULT_EVALS);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .failure()
        .stderr(contains(
            "eval 'e1': no effective codebase; set top-level 'codebase' or this eval's 'codebase'",
        ));

    assert!(!iteration_dir(&cwd).exists());
}

#[test]
fn retired_isolation_field_fails_with_migration_guidance() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review", "expected_output": "a review", "isolation": "isolated" }
    ] }"#;
    let (skill_dir, cwd) = setup_raw(tmp.path(), evals);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .failure()
        .stderr(contains(
            "field 'isolation' is no longer supported or needed; every eval run already uses a private environment",
        ));

    assert!(!iteration_dir(&cwd).exists());
}
