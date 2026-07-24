use super::*;
use crate::core::EvalsConfig;
use serde_json::json;
use std::fs;

fn check(command: &str) -> AssertionCommandCheck {
    AssertionCommandCheck {
        id: "check".into(),
        setup_files: None,
        command: command.into(),
        expect_exit_code: 0,
        expect_stdout: None,
    }
}

#[cfg(unix)]
fn exit_command(code: i32) -> String {
    format!("exit {code}")
}

#[cfg(windows)]
fn exit_command(code: i32) -> String {
    format!("exit /B {code}")
}

#[cfg(unix)]
fn output_command() -> &'static str {
    "printf 'hello world\\n'; printf 'diagnostic\\n' >&2"
}

#[cfg(windows)]
fn output_command() -> &'static str {
    "echo hello world & echo diagnostic 1>&2"
}

#[cfg(unix)]
fn append_command() -> &'static str {
    "test -f holdout/secret.txt && printf x >> command-runs.txt"
}

#[cfg(windows)]
fn append_command() -> &'static str {
    "if exist holdout\\secret.txt (echo x>>command-runs.txt) else (exit /B 1)"
}

fn evals(command: &str) -> EvalsConfig {
    serde_json::from_value(json!({
        "skill_name": "demo",
        "evals": [{
            "id": "e1",
            "prompt": "p",
            "expected_output": "o",
            "assertions": [{
                "id": "check",
                "type": "command_check",
                "setup_files": ["holdout/secret.txt"],
                "command": command
            }]
        }]
    }))
    .unwrap()
}

fn write_dispatch(iteration_dir: &Path, eval_root: &Path, shared: bool) {
    fs::create_dir_all(iteration_dir).unwrap();
    let mut tasks = vec![json!({
        "eval_id": "e1",
        "condition": "with_skill",
        "eval_root": eval_root,
        "run_record_path": iteration_dir.join("eval-e1/with_skill/run.json")
    })];
    if shared {
        tasks.push(json!({
            "eval_id": "e2",
            "condition": "with_skill",
            "eval_root": eval_root,
            "run_record_path": iteration_dir.join("eval-e2/with_skill/run.json")
        }));
    }
    fs::write(
        iteration_dir.join("dispatch.json"),
        serde_json::to_vec(&json!({ "tasks": tasks })).unwrap(),
    )
    .unwrap();
}

#[test]
fn expected_and_unexpected_exit_codes_are_assertion_results() {
    let root = tempfile::TempDir::new().unwrap();

    let passing = execute_command_check(&check(&exit_command(0)), root.path()).unwrap();
    assert!(passing.passed);
    assert_eq!(passing.actual_exit_code, Some(0));

    let mut unexpected = check(&exit_command(3));
    unexpected.expect_exit_code = 0;
    let failed = execute_command_check(&unexpected, root.path()).unwrap();
    assert!(!failed.passed);
    assert_eq!(failed.actual_exit_code, Some(3));
    assert!(failed.evidence.contains("expected exit code 0"));
    assert!(failed.evidence.contains("got 3"));
}

#[test]
fn stdout_regex_must_match_complete_lossy_stdout() {
    let root = tempfile::TempDir::new().unwrap();
    let mut passing = check(output_command());
    passing.expect_stdout = Some("hello\\s+world".into());
    assert!(execute_command_check(&passing, root.path()).unwrap().passed);

    passing.expect_stdout = Some("^goodbye$".into());
    let failed = execute_command_check(&passing, root.path()).unwrap();
    assert!(!failed.passed);
    assert!(failed.evidence.contains("stdout did not match"));
}

#[test]
fn invalid_stdout_regex_is_a_failed_assertion_with_evidence() {
    let root = tempfile::TempDir::new().unwrap();
    let mut assertion = check(output_command());
    assertion.expect_stdout = Some("(".into());

    let result = execute_command_check(&assertion, root.path()).unwrap();
    assert!(!result.passed);
    assert!(result.evidence.contains("invalid expect_stdout regex"));
}

#[test]
fn stdout_and_stderr_diagnostics_are_retained_and_capped_at_two_kib() {
    let root = tempfile::TempDir::new().unwrap();
    let result = execute_command_check(&check(output_command()), root.path()).unwrap();
    assert!(result.stdout.contains("hello world"));
    assert!(result.stderr.contains("diagnostic"));
    assert!(result.stdout.len() <= 2048);
    assert!(result.stderr.len() <= 2048);
}

#[cfg(unix)]
#[test]
fn stdout_regex_uses_complete_output_before_diagnostics_are_truncated() {
    let root = tempfile::TempDir::new().unwrap();
    let mut assertion =
        check("i=0; while [ \"$i\" -lt 3000 ]; do printf x; i=$((i + 1)); done; printf TAIL");
    assertion.expect_stdout = Some("TAIL$".into());
    let result = execute_command_check(&assertion, root.path()).unwrap();
    assert!(result.passed);
    assert_eq!(result.stdout.len(), 2048);
    assert!(!result.stdout.contains("TAIL"));
}

#[cfg(unix)]
#[test]
fn signal_termination_is_an_ordinary_failed_assertion() {
    let root = tempfile::TempDir::new().unwrap();
    let result = execute_command_check(&check("kill -TERM $$"), root.path()).unwrap();
    assert!(!result.passed);
    assert_eq!(result.actual_exit_code, None);
    assert!(result.evidence.contains("terminated by signal"));
}

#[test]
fn persisted_results_are_reused_and_overwrite_reruns_in_declaration_order() {
    let root = tempfile::TempDir::new().unwrap();
    let skill_dir = root.path().join("skill");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    fs::create_dir_all(skill_dir.join("evals/holdout")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    fs::write(skill_dir.join("evals/holdout/secret.txt"), "held out").unwrap();
    write_dispatch(&iteration_dir, &eval_root, false);
    assert!(!eval_root.join("holdout/secret.txt").exists());

    let first =
        grade_command_checks(&iteration_dir, &evals(append_command()), &skill_dir, false).unwrap();
    assert_eq!(first.executed, 1);
    assert_eq!(first.reused, 0);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "x"
    );
    assert_eq!(
        fs::read_to_string(eval_root.join("holdout/secret.txt")).unwrap(),
        "held out"
    );
    let result_path = iteration_dir.join("eval-e1/with_skill/command-checks/check.json");
    assert!(result_path.exists());

    let reused =
        grade_command_checks(&iteration_dir, &evals(append_command()), &skill_dir, false).unwrap();
    assert_eq!(reused.executed, 0);
    assert_eq!(reused.reused, 1);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "x"
    );

    let overwritten =
        grade_command_checks(&iteration_dir, &evals(append_command()), &skill_dir, true).unwrap();
    assert_eq!(overwritten.executed, 1);
    assert_eq!(overwritten.reused, 0);
    assert_eq!(
        fs::read_to_string(eval_root.join("command-runs.txt")).unwrap(),
        "xx"
    );
}

#[test]
fn shared_eval_root_is_rejected_with_fresh_iteration_guidance() {
    let root = tempfile::TempDir::new().unwrap();
    let skill_dir = root.path().join("skill");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    fs::create_dir_all(skill_dir.join("evals/holdout")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    fs::write(skill_dir.join("evals/holdout/secret.txt"), "held out").unwrap();
    write_dispatch(&iteration_dir, &eval_root, true);

    let error = grade_command_checks(&iteration_dir, &evals("true"), &skill_dir, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("shares eval_root"), "{error}");
    assert!(error.contains("fresh iteration"), "{error}");
}

#[cfg(unix)]
#[test]
fn multiple_checks_execute_in_declaration_order_against_one_env() {
    let root = tempfile::TempDir::new().unwrap();
    let skill_dir = root.path().join("skill");
    let iteration_dir = root.path().join("iteration-1");
    let eval_root = iteration_dir.join("env-g1-with_skill");
    fs::create_dir_all(skill_dir.join("evals")).unwrap();
    fs::create_dir_all(&eval_root).unwrap();
    write_dispatch(&iteration_dir, &eval_root, false);
    let evals: EvalsConfig = serde_json::from_value(json!({
        "skill_name": "demo",
        "evals": [{
            "id": "e1",
            "prompt": "p",
            "expected_output": "o",
            "assertions": [
                {
                    "id": "first",
                    "type": "command_check",
                    "command": "printf ready > state.txt"
                },
                {
                    "id": "second",
                    "type": "command_check",
                    "command": "test \"$(cat state.txt)\" = ready"
                }
            ]
        }]
    }))
    .unwrap();

    let summary = grade_command_checks(&iteration_dir, &evals, &skill_dir, false).unwrap();
    assert_eq!(summary.executed, 2);
    for id in ["first", "second"] {
        let result: CommandCheckResult = serde_json::from_str(
            &fs::read_to_string(
                iteration_dir.join(format!("eval-e1/with_skill/command-checks/{id}.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(result.passed, "{id}: {}", result.evidence);
    }
}
