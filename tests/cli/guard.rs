//! The hidden `guard` PreToolUse hook entry point and `teardown-guard`.

use crate::helpers::skill_eval;
use predicates::prelude::*;
use predicates::str::contains;
use std::fs;
use tempfile::TempDir;

/// The internal `guard` hook entry point is hidden from `--help` (its unique
/// description never appears) yet remains callable.
#[test]
fn guard_subcommand_is_hidden_but_callable() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("PreToolUse hook entry point").not());

    skill_eval().arg("guard").arg("--help").assert().success();
}

/// Write an armed guard marker scoping writes to `<allowed>`, and return its path.
fn write_armed_marker(root: &std::path::Path, allowed: &std::path::Path) -> std::path::PathBuf {
    let skills = root.join(".claude").join("skills");
    fs::create_dir_all(&skills).unwrap();
    let marker = skills.join(".slow-powers-eval-guard.json");
    fs::write(
        &marker,
        format!(
            r#"{{ "active": true, "allowedRoots": ["{}"], "expiresAt": "2999-01-01T00:00:00.000Z" }}"#,
            allowed.display()
        ),
    )
    .unwrap();
    marker
}

/// `guard` denies a Write outside the sandbox: it prints a PreToolUse deny verdict
/// on stdout and still exits 0 (the hook must never fail the session).
#[test]
fn guard_denies_out_of_bounds_write() {
    let tmp = TempDir::new().unwrap();
    let marker = write_armed_marker(tmp.path(), &tmp.path().join("skills-workspace"));

    skill_eval()
        .arg("guard")
        .arg(&marker)
        .write_stdin(r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#)
        .assert()
        .success()
        .stdout(contains(r#""permissionDecision":"deny""#));
}

/// `guard` allows an in-bounds write: empty stdout, exit 0.
#[test]
fn guard_allows_in_bounds_write() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("skills-workspace");
    let marker = write_armed_marker(tmp.path(), &workspace);

    skill_eval()
        .arg("guard")
        .arg(&marker)
        .write_stdin(format!(
            r#"{{ "tool_name": "Write", "tool_input": {{ "file_path": "{}/out.md" }} }}"#,
            workspace.display()
        ))
        .assert()
        .success()
        .stdout("");
}

/// `guard` fails open when the marker is absent: empty stdout, exit 0.
#[test]
fn guard_fails_open_without_marker() {
    let tmp = TempDir::new().unwrap();
    skill_eval()
        .arg("guard")
        .arg(tmp.path().join("nope.json"))
        .write_stdin(r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#)
        .assert()
        .success()
        .stdout("");
}

/// `teardown-guard` reports when no guard is installed (cwd has no marker).
#[test]
fn teardown_guard_reports_nothing_to_remove() {
    let tmp = TempDir::new().unwrap();
    skill_eval()
        .arg("teardown-guard")
        .current_dir(tmp.path())
        .assert()
        .success()
        .stdout(contains("nothing to remove"));
}

/// `teardown-guard` sweeps a stray marker in the cwd and reports removal.
#[test]
fn teardown_guard_removes_installed_guard() {
    let tmp = TempDir::new().unwrap();
    write_armed_marker(tmp.path(), &tmp.path().join("skills-workspace"));

    skill_eval()
        .arg("teardown-guard")
        .current_dir(tmp.path())
        .assert()
        .success()
        .stdout(contains("Write guard removed"));

    assert!(
        !tmp.path()
            .join(".claude/skills/.slow-powers-eval-guard.json")
            .exists()
    );
}
