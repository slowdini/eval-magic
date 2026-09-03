use super::*;

#[test]
fn default_run_auto_arms_guard_in_each_env() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // No guard flag at all: the harness declares guard support, so the run
    // arms it automatically (#126 — enhancements are provided, not opted into).
    let assert = skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("guard: armed"),
        "the run plan reports the armed guard: {stdout}"
    );
    let iteration = iteration_dir(&cwd);
    let conditions = read_json(&iteration.join("conditions.json"));
    let dispatch = read_json(&iteration.join("dispatch.json"));
    assert_eq!(conditions["guard_armed"], true);
    assert_eq!(dispatch["guard"], conditions["guard_armed"]);
    for condition in ["with_skill", "without_skill"] {
        let env = cli_env_dir(&cwd, "g1", condition);
        assert!(
            env.join(".claude/settings.local.json").exists(),
            "guard hook staged in env-g1-{condition}"
        );
        assert!(
            env.join(".claude/skills/.slow-powers-eval-guard.json")
                .exists(),
            "guard marker armed in env-g1-{condition}"
        );
    }
}

#[test]
fn no_guard_run_installs_no_guard() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--no-guard"])
        .assert()
        .success();
    let env = cli_env_dir(&cwd, "g1", "with_skill");
    assert!(!env.join(".claude/settings.local.json").exists());
    assert!(
        !env.join(".claude/skills/.slow-powers-eval-guard.json")
            .exists()
    );
    let iteration = iteration_dir(&cwd);
    let conditions = read_json(&iteration.join("conditions.json"));
    let dispatch = read_json(&iteration.join("dispatch.json"));
    assert_eq!(conditions["guard_armed"], false);
    assert_eq!(dispatch["guard"], conditions["guard_armed"]);
}
