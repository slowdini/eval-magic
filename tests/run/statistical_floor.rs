//! Pre-dispatch statistical-floor output for uniform and per-eval run counts.

use crate::helpers::*;

const ONE_RUN: &str = concat!(
    "statistical floor: 2 conditions × 1 run; minimum attainable ",
    "two-sided Fisher exact p on a binary endpoint is 1.0"
);
const THREE_RUNS: &str = concat!(
    "statistical floor: 2 conditions × 3 runs; minimum attainable ",
    "two-sided Fisher exact p on a binary endpoint is 0.10"
);

#[test]
fn runs_flag_prints_the_minimum_attainable_fisher_p_value() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--runs",
            "3",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains(THREE_RUNS));
}

#[test]
fn mixed_per_eval_run_counts_print_each_floor_once_in_order() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review MR 1", "expected_output": "a review", "runs": 3 },
        { "id": "e2", "prompt": "review MR 2", "expected_output": "a review" } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    let assert = skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert_eq!(stdout.matches(ONE_RUN).count(), 1, "{stdout}");
    assert_eq!(stdout.matches(THREE_RUNS).count(), 1, "{stdout}");
    assert!(stdout.find(ONE_RUN) < stdout.find(THREE_RUNS), "{stdout}");
}

#[test]
fn excluded_evals_do_not_influence_the_statistical_floor() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review MR 1", "expected_output": "a review" },
        { "id": "e2", "prompt": "review MR 2", "expected_output": "a review", "runs": 3 } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    let assert = skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--only",
            "e1",
            "--dry-run",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains(ONE_RUN), "{stdout}");
    assert!(
        !stdout.contains(THREE_RUNS),
        "excluded evals must not influence the notice: {stdout}"
    );
}

#[test]
fn sampled_judging_prints_the_non_binary_endpoint_instead_of_a_fisher_floor() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [{
        "id": "quality", "prompt": "review it", "expected_output": "a review",
        "assertions": [{
            "id": "design", "type": "llm_judge", "rubric": "Is it well designed?", "samples": 3
        }]
    }] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    let assert = skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(
        stdout.contains(
            "statistical endpoint: 2 conditions × 1 run; LLM judge sample counts per assertion: 3"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("vote proportion and pass^k; the binary Fisher exact floor does not apply"),
        "{stdout}"
    );
    assert!(!stdout.contains("statistical floor:"), "{stdout}");
}

#[test]
fn sampled_endpoint_resolves_run_default_and_assertion_overrides() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [{
        "id": "quality", "prompt": "review it", "expected_output": "a review",
        "assertions": [
            {"id": "defaulted", "type": "llm_judge", "rubric": "Good?"},
            {"id": "overridden", "type": "llm_judge", "rubric": "Safe?", "samples": 3}
        ]
    }] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    let assert = skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--judge-samples",
            "5",
            "--dry-run",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(
        stdout.contains("LLM judge sample counts per assertion: 3, 5"),
        "{stdout}"
    );
}
