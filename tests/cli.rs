//! Integration tests for the CLI surface, driving the built `skill-eval`
//! binary. Mirrors the subprocess-style integration tests in eval-runner
//! (`cli.test.ts`). These pin the command tree and dispatch behavior of the
//! Phase-0 scaffold; per-command behavior is tested as each module is ported.

use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use tempfile::TempDir;

fn skill_eval() -> Command {
    Command::cargo_bin("skill-eval").expect("binary `skill-eval` should build")
}

/// A minimal valid `evals.json` body.
const VALID_EVALS: &str = r#"{ "skill_name": "demo", "evals": [
    { "id": "e1", "prompt": "p", "expected_output": "o" } ] }"#;

/// Build `<root>/<skill>/evals/evals.json` with the given contents.
fn write_evals(root: &std::path::Path, skill: &str, contents: &str) {
    let dir = root.join(skill).join("evals");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("evals.json"), contents).unwrap();
}

/// `--help` succeeds and lists the subcommands ported from eval-runner.
#[test]
fn help_lists_subcommands() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("record-runs"))
        .stdout(contains("grade"))
        .stdout(contains("validate"))
        .stdout(contains("aggregate"));
}

/// The binary name in help output is the published command name.
#[test]
fn help_uses_published_binary_name() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("skill-eval"));
}

/// A recognized subcommand whose module hasn't been ported yet dispatches to a
/// handler that reports "not yet implemented" and exits non-zero.
#[test]
fn unported_subcommand_reports_not_yet_implemented() {
    skill_eval()
        .arg("grade")
        .assert()
        .failure()
        .stderr(contains("not yet implemented"));
}

/// `validate` over a dir of valid evals succeeds and prints a ✓ per file.
#[test]
fn validate_succeeds_on_valid_evals() {
    let tmp = TempDir::new().unwrap();
    write_evals(tmp.path(), "good", VALID_EVALS);

    skill_eval()
        .arg("validate")
        .arg("--skill-dir")
        .arg(tmp.path())
        .assert()
        .success()
        .stdout(contains("✓ good/evals/evals.json"))
        .stdout(contains("Validated 1 evals.json file(s); 0 failed."));
}

/// `validate` exits non-zero and prints a ✗ when a file fails validation.
#[test]
fn validate_fails_on_invalid_evals() {
    let tmp = TempDir::new().unwrap();
    write_evals(tmp.path(), "bad", r#"{ "skill_name": "x", "evals": [] }"#);

    skill_eval()
        .arg("validate")
        .arg("--skill-dir")
        .arg(tmp.path())
        .assert()
        .failure()
        .stderr(contains("✗"));
}

/// `validate` without the required `--skill-dir` flag fails with our message.
#[test]
fn validate_requires_skill_dir() {
    skill_eval()
        .arg("validate")
        .assert()
        .failure()
        .stderr(contains("missing required flag --skill-dir"));
}

/// An unknown subcommand is rejected by the parser (clap), not silently
/// accepted.
#[test]
fn unknown_subcommand_is_rejected() {
    skill_eval().arg("does-not-exist").assert().failure();
}
