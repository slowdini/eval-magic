//! Compatibility behavior for evals that do not declare a sourced codebase.

use crate::codebase_support::git;
use crate::helpers::*;

#[test]
fn a_fixture_only_eval_still_gets_the_repository_it_always_had() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let env = cli_env_dir(&cwd, "g1", "with_skill");
    assert_eq!(git(&env, &["symbolic-ref", "--short", "HEAD"]), "work");
    assert_eq!(git(&env, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(git(&env, &["remote"]), "");
    assert_eq!(git(&env, &["status", "--porcelain"]), "");
    assert_eq!(
        git(&env, &["rev-parse", "refs/eval-magic/baseline"]),
        git(&env, &["rev-parse", "HEAD"])
    );
}
