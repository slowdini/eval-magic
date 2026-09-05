use super::*;

/// Auto-arm never turns the user-only-descriptor restriction into an error:
/// without an explicit `--guard`, the run proceeds unguarded with a warning
/// naming the fallback.
#[test]
fn auto_guard_stays_off_without_error_on_user_only_harness() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    write_project_descriptor(
        &cwd,
        &format!("skills_dir = \".cool/skills\"\nconfig_dirs = [\".cool\"]\n{COOL_DESCRIPTOR}"),
    );

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--harness",
            "cool-custom-harness",
        ])
        .assert()
        .success()
        .stderr(contains("declares no write guard").and(contains("detect-stray-writes")));

    let iteration = iteration_dir(&cwd);
    let dispatch = read_json(&iteration.join("dispatch.json"));
    let conditions = read_json(&iteration.join("conditions.json"));
    assert_eq!(conditions["guard_armed"], false);
    assert_eq!(dispatch["guard"], conditions["guard_armed"]);
}
