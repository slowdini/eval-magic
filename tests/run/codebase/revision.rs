//! Revision-mode provisioning and promotion for codebase-backed iterations.

use super::*;

#[test]
fn revision_mode_provisions_both_arms_from_the_cached_codebase() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["snapshot", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--label", "baseline"])
        .assert()
        .success();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "revision",
            "--judge-samples",
            "3",
            "--dry-run",
        ])
        .assert()
        .success();

    let iteration = iteration_dir(&cwd);
    let conditions = read_json(&iteration.join("conditions.json"));
    assert_eq!(conditions["mode"], "revision");
    assert_eq!(conditions["judge_samples"], 3);
    assert_eq!(conditions["codebases"][0]["source"], wire_path(&origin));
    let revision = conditions["codebases"][0]["revision"]
        .as_str()
        .expect("revision mode records the resolved codebase SHA")
        .to_string();
    let cached: Vec<_> = fs::read_dir(iteration.join(".codebase")).unwrap().collect();
    assert_eq!(
        cached.len(),
        1,
        "both arms of the comparison share one cached materialization"
    );

    for condition in ["old_skill", "new_skill"] {
        let env = iteration.join(format!("env-g1-{condition}"));
        assert_eq!(
            fs::read_to_string(env.join("src/main.rs")).unwrap(),
            "fn main() {}\n",
            "{condition}: the codebase's files must be present"
        );
        assert!(
            git(&env, &["rev-list", "--count", "HEAD"])
                .parse::<u32>()
                .unwrap()
                >= 2,
            "{condition}: the history must survive provisioning"
        );
        assert_eq!(
            git(&env, &["remote"]),
            "",
            "{condition}: no env may retain a remote"
        );
        assert_eq!(
            git(&env, &["rev-parse", "refs/eval-magic/baseline"]),
            git(&env, &["rev-parse", "HEAD"]),
            "{condition}: the baseline still names the start state"
        );
    }

    // Promotion carries provenance from the revision-mode conditions record
    // into the durable baseline report.
    fs::write(
        iteration.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    )
    .unwrap();
    skill_eval()
        .current_dir(&cwd)
        .args(["promote-baseline", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .success();
    let baseline = read_str(&skill_dir.join("mr-review/evals/baseline/BASELINE.md"));
    assert!(baseline.contains(&wire_path(&origin)), "{baseline}");
    assert!(baseline.contains(&revision[..7]), "{baseline}");
}
