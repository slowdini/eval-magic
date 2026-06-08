//! Integration tests for the CLI surface, driving the built `skill-eval`
//! binary. Mirrors the subprocess-style integration tests in eval-runner
//! (`cli.test.ts`). These pin the command tree and dispatch behavior of the
//! Phase-0 scaffold; per-command behavior is tested as each module is ported.

use assert_cmd::Command;
use predicates::str::contains;

fn skill_eval() -> Command {
    Command::cargo_bin("skill-eval").expect("binary `skill-eval` should build")
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
        .arg("validate")
        .assert()
        .failure()
        .stderr(contains("not yet implemented"));
}

/// An unknown subcommand is rejected by the parser (clap), not silently
/// accepted.
#[test]
fn unknown_subcommand_is_rejected() {
    skill_eval().arg("does-not-exist").assert().failure();
}
