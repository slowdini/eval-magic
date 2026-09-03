use super::*;
use crate::core::EvalsConfig;
use crate::pipeline::grade::instrument::GradingInstrument;
use serde_json::json;
use std::fs;

mod lifecycle;
mod staleness;

fn check(command: &str) -> AssertionCommandCheck {
    AssertionCommandCheck {
        id: "check".into(),
        setup_files: None,
        command: command.into(),
        env: None,
        matrix: None,
        expect_exit_code: 0,
        expect_stdout: None,
    }
}

/// A `__fixture` invocation as a shell command line.
///
/// `execute_command_check` hands the string to `sh -c`. Double-quoting the
/// program path and arguments preserves spaces and literal fixture values.
fn fixture(args: &[&str]) -> String {
    let exe = assert_cmd::cargo::cargo_bin("eval-magic");
    assert!(
        exe.is_file(),
        "the __fixture command needs the eval-magic binary at {}; \
         run `cargo test`, which builds bins, or `cargo build` first",
        exe.display()
    );
    let mut command = format!("\"{}\" __fixture", exe.display());
    for arg in args {
        command.push_str(&format!(" \"{arg}\""));
    }
    command
}

fn exit_command(code: i32) -> String {
    fixture(&["--exit", &code.to_string()])
}

fn output_command() -> String {
    fixture(&["--text", "hello world", "--stderr", "diagnostic"])
}

/// Echoes the override so the test can assert the child saw it. The `PATH`
/// requirement keeps the check honest: a child handed an empty environment
/// would echo an empty value and look like a pass.
fn environment_output_command() -> String {
    fixture(&[
        "--require-env",
        "PATH",
        "--echo-env",
        "EVAL_MAGIC_TEST_VALUE",
    ])
}

fn append_command() -> String {
    fixture(&[
        "--require-file",
        "holdout/secret.txt",
        "--text",
        "x",
        "--append",
        "command-runs.txt",
    ])
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

/// Run every command check in `config` against the iteration, resolving
/// held-out setup files from `skill_dir`. No live tree, so nothing refreshes.
fn grade_frozen(
    iteration_dir: &Path,
    config: EvalsConfig,
    skill_dir: &Path,
    overwrite: bool,
) -> CommandCheckSummary {
    grade_command_checks(
        iteration_dir,
        &GradingInstrument::frozen(config, skill_dir),
        overwrite,
    )
    .unwrap()
}

fn write_dispatch(iteration_dir: &Path, eval_root: &Path, shared: bool) {
    fs::create_dir_all(iteration_dir).unwrap();
    let first_run = iteration_dir.join("eval-e1/with_skill/run.json");
    let mut tasks = vec![json!({
        "eval_id": "e1",
        "condition": "with_skill",
        "eval_root": eval_root,
        "run_record_path": first_run
    })];
    let mut run_records = vec![(first_run, json!({"run": "first"}))];
    if shared {
        let second_run = iteration_dir.join("eval-e2/with_skill/run.json");
        tasks.push(json!({
            "eval_id": "e2",
            "condition": "with_skill",
            "eval_root": eval_root,
            "run_record_path": second_run
        }));
        run_records.push((second_run, json!({"run": "second"})));
    }
    fs::write(
        iteration_dir.join("dispatch.json"),
        serde_json::to_vec(&json!({ "tasks": tasks })).unwrap(),
    )
    .unwrap();
    for (path, record) in run_records {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    }
}

#[test]
fn expected_and_unexpected_exit_codes_are_assertion_results() {
    let root = tempfile::TempDir::new().unwrap();

    let passing = execute_command_check(&check(&exit_command(0)), root.path()).unwrap();
    assert!(passing.passed);
    assert_eq!(passing.actual_exit_code, Some(0));
    assert!(
        serde_json::to_value(&passing)
            .unwrap()
            .get("cells")
            .is_none(),
        "non-matrix result shape stays unchanged"
    );

    let mut unexpected = check(&exit_command(3));
    unexpected.expect_exit_code = 0;
    let failed = execute_command_check(&unexpected, root.path()).unwrap();
    assert!(!failed.passed);
    assert_eq!(failed.actual_exit_code, Some(3));
    assert!(failed.evidence.contains("expected exit code 0"));
    assert!(failed.evidence.contains("got 3"));
}

/// An eval author's `command_check` reaches `sh -c` with its own quoting intact.
#[test]
fn command_reaches_the_posix_shell_with_its_quoting_intact() {
    let root = tempfile::TempDir::new().unwrap();
    let result =
        execute_command_check(&check(&fixture(&["--text", "spaced value"])), root.path()).unwrap();
    assert!(result.passed, "{}", result.evidence);
    assert_eq!(result.stdout, "spaced value");
}

#[test]
fn command_accepts_posix_environment_assignment_syntax() {
    let root = tempfile::TempDir::new().unwrap();
    let command = format!(
        "EVAL_MAGIC_TEST_VALUE=posix {}",
        fixture(&["--require-env", "EVAL_MAGIC_TEST_VALUE=posix"])
    );

    let result = execute_command_check(&check(&command), root.path()).unwrap();

    assert!(result.passed, "{}", result.evidence);
}

#[test]
fn stdout_regex_must_match_complete_lossy_stdout() {
    let root = tempfile::TempDir::new().unwrap();
    let mut passing = check(&output_command());
    passing.expect_stdout = Some("hello\\s+world".into());
    assert!(execute_command_check(&passing, root.path()).unwrap().passed);

    passing.expect_stdout = Some("^goodbye$".into());
    let failed = execute_command_check(&passing, root.path()).unwrap();
    assert!(!failed.passed);
    assert!(failed.evidence.contains("stdout did not match"));
}

#[test]
fn command_environment_overrides_are_visible_to_the_child_process() {
    let root = tempfile::TempDir::new().unwrap();
    let mut assertion = check(&environment_output_command());
    assertion.env = Some(std::collections::BTreeMap::from([(
        "EVAL_MAGIC_TEST_VALUE".into(),
        "configured".into(),
    )]));
    assertion.expect_stdout = Some("^configured$".into());

    let result = execute_command_check(&assertion, root.path()).unwrap();
    assert!(result.passed, "{}", result.evidence);
    assert_eq!(result.stdout, "configured");
}

#[test]
fn command_checks_clear_inherited_git_routing_before_explicit_overlays() {
    const CHILD_MARKER: &str = "EVAL_MAGIC_GIT_ENV_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        let root = tempfile::TempDir::new().unwrap();
        // `--default unset` distinguishes a cleared variable from one set to the
        // empty string, which is the property `clear_git_environment` promises.
        let inherited = execute_command_check(
            &check(&fixture(&["--echo-env", "GIT_DIR", "--default", "unset"])),
            root.path(),
        )
        .unwrap();
        assert_eq!(inherited.stdout, "unset");

        let mut restored = check(&fixture(&["--echo-env", "GIT_DIR"]));
        restored.env = Some(std::collections::BTreeMap::from([(
            "GIT_DIR".into(),
            "/declared/repository.git".into(),
        )]));
        let restored = execute_command_check(&restored, root.path()).unwrap();
        assert_eq!(restored.stdout, "/declared/repository.git");
        return;
    }

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "pipeline::grade::command_check::tests::command_checks_clear_inherited_git_routing_before_explicit_overlays",
            "--nocapture",
        ])
        .env(CHILD_MARKER, "1")
        .env("GIT_DIR", "/inherited/repository.git")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child test failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn invalid_direct_environment_configuration_returns_an_error_instead_of_panicking() {
    let root = tempfile::TempDir::new().unwrap();
    let mut assertion = check(&exit_command(0));
    assertion.env = Some(std::collections::BTreeMap::from([(
        "BAD=NAME".into(),
        "value".into(),
    )]));

    let error = execute_command_check(&assertion, root.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("environment variable name"), "{error}");
    assert!(error.contains("BAD=NAME"), "{error}");

    assertion.env = None;
    assertion.matrix = Some(std::collections::BTreeMap::from([(
        "VALID_NAME".into(),
        vec!["bad\u{0}value".into()],
    )]));
    let error = execute_command_check(&assertion, root.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("matrix"), "{error}");
    assert!(error.contains("value must not contain NUL"), "{error}");
}

#[test]
fn stdout_expectation_is_applied_to_every_matrix_cell() {
    let root = tempfile::TempDir::new().unwrap();
    let mut assertion = check(&environment_output_command());
    assertion.matrix = Some(std::collections::BTreeMap::from([(
        "EVAL_MAGIC_TEST_VALUE".into(),
        vec!["expected".into(), "unexpected".into()],
    )]));
    assertion.expect_stdout = Some("^expected$".into());

    let result = execute_command_check(&assertion, root.path()).unwrap();
    assert!(!result.passed);
    assert!(result.evidence.contains("1/2 matrix cells passed"));
    let cells = result.cells.as_ref().unwrap();
    assert!(cells[0].passed);
    assert!(!cells[1].passed);
    assert!(
        cells[1]
            .evidence
            .contains("stdout did not match expect_stdout")
    );
}

#[test]
fn matrix_environment_expansion_is_deterministic_and_overrides_fixed_values() {
    let mut assertion = check(&exit_command(0));
    assertion.env = Some(std::collections::BTreeMap::from([
        ("FIXED".into(), "configured".into()),
        ("TZ".into(), "base".into()),
    ]));
    assertion.matrix = Some(std::collections::BTreeMap::from([
        ("LOCALE".into(), vec!["en_US".into(), "de_DE".into()]),
        ("TZ".into(), vec!["UTC".into(), "Europe/Berlin".into()]),
    ]));

    let cells = matrix_environments(&assertion);
    assert_eq!(
        cells
            .iter()
            .map(|cell| (
                cell["FIXED"].as_str(),
                cell["LOCALE"].as_str(),
                cell["TZ"].as_str(),
            ))
            .collect::<Vec<_>>(),
        vec![
            ("configured", "en_US", "UTC"),
            ("configured", "en_US", "Europe/Berlin"),
            ("configured", "de_DE", "UTC"),
            ("configured", "de_DE", "Europe/Berlin"),
        ]
    );
}

#[test]
fn matrix_runs_every_cartesian_cell_in_deterministic_order_and_reports_results() {
    let root = tempfile::TempDir::new().unwrap();
    // The append must happen for every cell, including the ones that fail —
    // `matrix-runs.txt` is the evidence that all four ran, and in what order.
    let mut assertion = check(&fixture(&[
        "--echo-env",
        "FIXED",
        "--echo-env",
        "LOCALE",
        "--echo-env",
        "TZ",
        "--separator",
        "|",
        "--newline",
        "--append",
        "matrix-runs.txt",
        "--require-env",
        "TZ=UTC",
    ]));
    assertion.env = Some(std::collections::BTreeMap::from([
        ("FIXED".into(), "configured".into()),
        ("TZ".into(), "base".into()),
    ]));
    assertion.matrix = Some(std::collections::BTreeMap::from([
        ("LOCALE".into(), vec!["en_US".into(), "de_DE".into()]),
        ("TZ".into(), vec!["UTC".into(), "Europe/Berlin".into()]),
    ]));

    let result = execute_command_check(&assertion, root.path()).unwrap();
    assert!(!result.passed);
    assert_eq!(result.actual_exit_code, None);
    assert_eq!(result.stdout, "");
    assert_eq!(result.stderr, "");
    assert!(result.evidence.contains("2/4 matrix cells passed"));
    assert!(result.evidence.contains("TZ=Europe/Berlin"));

    let cells = result.cells.as_ref().unwrap();
    assert_eq!(cells.len(), 4);
    assert_eq!(
        cells
            .iter()
            .map(|cell| (
                cell.env["FIXED"].as_str(),
                cell.env["LOCALE"].as_str(),
                cell.env["TZ"].as_str(),
                cell.passed,
            ))
            .collect::<Vec<_>>(),
        vec![
            ("configured", "en_US", "UTC", true),
            ("configured", "en_US", "Europe/Berlin", false),
            ("configured", "de_DE", "UTC", true),
            ("configured", "de_DE", "Europe/Berlin", false),
        ]
    );
    assert_eq!(
        fs::read_to_string(root.path().join("matrix-runs.txt")).unwrap(),
        "configured|en_US|UTC\n\
         configured|en_US|Europe/Berlin\n\
         configured|de_DE|UTC\n\
         configured|de_DE|Europe/Berlin\n"
    );
}

#[test]
fn invalid_stdout_regex_is_a_failed_assertion_with_evidence() {
    let root = tempfile::TempDir::new().unwrap();
    let mut assertion = check(&output_command());
    assertion.expect_stdout = Some("(".into());

    let result = execute_command_check(&assertion, root.path()).unwrap();
    assert!(!result.passed);
    assert!(result.evidence.contains("invalid expect_stdout regex"));
}

#[test]
fn stdout_and_stderr_diagnostics_are_retained_and_capped_at_two_kib() {
    let root = tempfile::TempDir::new().unwrap();
    let result = execute_command_check(&check(&output_command()), root.path()).unwrap();
    assert!(result.stdout.contains("hello world"));
    assert!(result.stderr.contains("diagnostic"));
    assert!(result.stdout.len() <= 2048);
    assert!(result.stderr.len() <= 2048);
}

#[test]
fn stdout_regex_uses_complete_output_before_diagnostics_are_truncated() {
    let root = tempfile::TempDir::new().unwrap();
    let mut assertion = check(&fixture(&["--pad", "3000", "--text", "TAIL"]));
    assertion.expect_stdout = Some("TAIL$".into());
    let result = execute_command_check(&assertion, root.path()).unwrap();
    assert!(result.passed);
    assert_eq!(result.stdout.len(), 2048);
    assert!(!result.stdout.contains("TAIL"));
}

/// The evidence wording is host-independent even though reading a signal is
/// not, so it is pinned on every platform rather than only where signals exist.
#[test]
fn termination_message_names_the_signal_when_there_is_one() {
    assert_eq!(
        termination_message(Some(15)),
        "command terminated by signal 15"
    );
    assert_eq!(
        termination_message(None),
        "command terminated without an exit code"
    );
}

#[test]
fn signal_termination_is_an_ordinary_failed_assertion() {
    let root = tempfile::TempDir::new().unwrap();
    let result = execute_command_check(&check("kill -TERM $$"), root.path()).unwrap();
    assert!(!result.passed);
    assert_eq!(result.actual_exit_code, None);
    assert!(result.evidence.contains("terminated by signal"));
}
