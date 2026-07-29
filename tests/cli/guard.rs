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
    skill_eval()
        .args(["guard-hook", "--help"])
        .assert()
        .success();
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

fn write_codex_armed_marker(
    root: &std::path::Path,
    allowed: &std::path::Path,
) -> std::path::PathBuf {
    let skills = root.join(".agents").join("skills");
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
    let marker = write_armed_marker(tmp.path(), &tmp.path().join(".eval-magic"));

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
    let workspace = tmp.path().join(".eval-magic");
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

/// Shell idioms agents reach for reflexively — `2>&1` duplicates a descriptor
/// and `/dev/null` names a stream — open no file, so the guard must let them
/// through end to end rather than costing the dispatch a retry.
#[test]
fn guard_allows_fd_duplication_and_the_null_device() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join(".eval-magic");
    fs::create_dir_all(&workspace).unwrap();
    let marker = write_armed_marker(tmp.path(), &workspace);

    for command in [
        "ls fixtures 2>/dev/null",
        "git status 2>&1 | head -20",
        "find . -type f 2>/dev/null | head -50",
        "printf done > out.txt 2>&1",
    ] {
        let out = skill_eval()
            .arg("guard")
            .arg(&marker)
            .write_stdin(format!(
                r#"{{ "tool_name": "Bash", "cwd": "{}", "tool_input": {{ "command": "{command}" }} }}"#,
                workspace.display()
            ))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
        assert!(
            stdout.is_empty(),
            "{command} should be allowed, got: {stdout}"
        );
    }
}

#[test]
fn guard_codex_subcommand_blocks_with_codex_verdict_shape() {
    let tmp = TempDir::new().unwrap();
    let marker = write_codex_armed_marker(tmp.path(), &tmp.path().join(".eval-magic"));

    skill_eval()
        .arg("guard-codex")
        .arg(&marker)
        .write_stdin(
            r#"{ "tool_name": "Bash", "tool_input": { "command": "npm install left-pad" } }"#,
        )
        .assert()
        .success()
        .stdout(contains(r#""decision":"block""#))
        .stdout(contains("blocked Bash"));
}

/// Byte-pin of the Claude deny verdict: armed hooks from previous releases keep
/// reading this exact shape, so the serialized bytes are a compatibility
/// contract — full-string equality, not substrings.
#[test]
fn guard_deny_verdict_bytes_are_stable() {
    let tmp = TempDir::new().unwrap();
    let marker = tmp.path().join("marker.json");
    fs::write(
        &marker,
        r#"{"active":true,"allowedRoots":["/work/env"],"expiresAt":"2999-01-01T00:00:00.000Z"}"#,
    )
    .unwrap();

    skill_eval()
        .arg("guard")
        .arg(&marker)
        .write_stdin(r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#)
        .assert()
        .success()
        .stdout(
            "{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\
             \"permissionDecision\":\"deny\",\"permissionDecisionReason\":\
             \"eval guard: Write to /etc/passwd is outside the eval sandbox \
             (allowed: /work/env)\"}}",
        );
}

/// Byte-pin of the Codex block verdict — same compatibility contract as the
/// Claude pin above.
#[test]
fn guard_codex_block_verdict_bytes_are_stable() {
    let tmp = TempDir::new().unwrap();
    let marker = tmp.path().join("marker.json");
    fs::write(
        &marker,
        r#"{"active":true,"allowedRoots":["/work/env"],"expiresAt":"2999-01-01T00:00:00.000Z"}"#,
    )
    .unwrap();

    skill_eval()
        .arg("guard-codex")
        .arg(&marker)
        .write_stdin(
            r#"{ "tool_name": "Bash", "tool_input": { "command": "npm install left-pad" } }"#,
        )
        .assert()
        .success()
        .stdout(
            "{\"decision\":\"block\",\"reason\":\"eval guard: blocked Bash \
             (package install/add) — runs outside the eval sandbox\"}",
        );
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

/// The generic `guard-hook --harness <name>` entry point resolves the verdict
/// shape from the named harness's embedded descriptor — Claude's deny shape
/// for claude-code, Codex's block shape for codex. Future guard-capable
/// built-ins use this instead of minting another alias.
#[test]
fn guard_hook_resolves_the_harness_verdict_shape() {
    let tmp = TempDir::new().unwrap();
    let marker = write_armed_marker(tmp.path(), &tmp.path().join(".eval-magic"));

    skill_eval()
        .args(["guard-hook", "--harness", "claude-code"])
        .arg(&marker)
        .write_stdin(r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#)
        .assert()
        .success()
        .stdout(contains(r#""permissionDecision":"deny""#));

    skill_eval()
        .args(["guard-hook", "--harness", "codex"])
        .arg(&marker)
        .write_stdin(
            r#"{ "tool_name": "Bash", "tool_input": { "command": "npm install left-pad" } }"#,
        )
        .assert()
        .success()
        .stdout(contains(r#""decision":"block""#));
}

/// `guard-hook` fails open on an unknown harness: empty stdout, exit 0 — the
/// hook must never brick a session, whatever config invoked it.
#[test]
fn guard_hook_fails_open_for_an_unknown_harness() {
    let tmp = TempDir::new().unwrap();
    let marker = write_armed_marker(tmp.path(), &tmp.path().join(".eval-magic"));

    skill_eval()
        .args(["guard-hook", "--harness", "mystery"])
        .arg(&marker)
        .write_stdin(r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#)
        .assert()
        .success()
        .stdout("");
}

fn write_opencode_armed_marker(
    root: &std::path::Path,
    allowed: &std::path::Path,
) -> std::path::PathBuf {
    let skills = root.join(".opencode").join("skills");
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

/// `guard-hook --harness opencode` round-trip, fed the exact payload shape
/// the staged plugin forwards: OpenCode's lowercase tool ids with camelCase
/// args. An out-of-bounds `write` blocks; an in-bounds one allows (empty
/// stdout).
#[test]
fn guard_hook_opencode_round_trips_write_verdicts() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join(".eval-magic");
    let marker = write_opencode_armed_marker(tmp.path(), &workspace);

    skill_eval()
        .args(["guard-hook", "--harness", "opencode"])
        .arg(&marker)
        .write_stdin(r#"{ "tool_name": "write", "tool_input": { "filePath": "/etc/passwd" } }"#)
        .assert()
        .success()
        .stdout(contains(r#""decision":"block""#));

    skill_eval()
        .args(["guard-hook", "--harness", "opencode"])
        .arg(&marker)
        .write_stdin(format!(
            r#"{{ "tool_name": "write", "tool_input": {{ "filePath": "{}/out.md" }} }}"#,
            workspace.display()
        ))
        .assert()
        .success()
        .stdout("");
}

/// The plugin file itself is protected: a bash call mutating anything under
/// `.opencode` trips the config-dir tamper rule.
#[test]
fn guard_hook_opencode_blocks_bash_tampering_with_the_plugin() {
    let tmp = TempDir::new().unwrap();
    let marker = write_opencode_armed_marker(tmp.path(), &tmp.path().join(".eval-magic"));

    skill_eval()
        .args(["guard-hook", "--harness", "opencode"])
        .arg(&marker)
        .write_stdin(
            r#"{ "tool_name": "bash", "tool_input": { "command": "touch .opencode/plugins/slow-powers-eval-guard.js" } }"#,
        )
        .assert()
        .success()
        .stdout(contains(r#""decision":"block""#));
}

/// Byte-pin of the OpenCode block verdict — same compatibility contract as
/// the Claude/Codex pins above: the staged plugin parses `decision`/`reason`
/// out of these exact bytes.
#[test]
fn guard_hook_opencode_block_verdict_bytes_are_stable() {
    let tmp = TempDir::new().unwrap();
    let marker = tmp.path().join("marker.json");
    fs::write(
        &marker,
        r#"{"active":true,"allowedRoots":["/work/env"],"expiresAt":"2999-01-01T00:00:00.000Z"}"#,
    )
    .unwrap();

    skill_eval()
        .args(["guard-hook", "--harness", "opencode"])
        .arg(&marker)
        .write_stdin(r#"{ "tool_name": "write", "tool_input": { "filePath": "/etc/passwd" } }"#)
        .assert()
        .success()
        .stdout(
            "{\"decision\":\"block\",\"reason\":\"eval guard: write to /etc/passwd is \
             outside the eval sandbox (allowed: /work/env)\"}",
        );
}

/// `teardown-guard` removes the staged plugin, sweeps the marker/manifest,
/// and prunes the plugins dir the install created.
#[test]
fn teardown_guard_removes_an_installed_opencode_plugin() {
    let tmp = TempDir::new().unwrap();
    let skills = tmp.path().join(".opencode").join("skills");
    fs::create_dir_all(&skills).unwrap();
    let marker = skills.join(".slow-powers-eval-guard.json");
    fs::write(
        &marker,
        r#"{ "active": true, "allowedRoots": [], "expiresAt": "2999-01-01T00:00:00.000Z" }"#,
    )
    .unwrap();
    let plugins = tmp.path().join(".opencode").join("plugins");
    fs::create_dir_all(&plugins).unwrap();
    let plugin = plugins.join("slow-powers-eval-guard.js");
    fs::write(&plugin, "// staged plugin\n").unwrap();
    fs::write(
        skills.join(".slow-powers-eval-guard-manifest.json"),
        format!(
            r#"{{ "created_at": "2026-07-23T00:00:00.000Z", "settings_path": "{}", "settings_existed": false, "settings_backup": null, "marker_path": "{}" }}"#,
            plugin.display(),
            marker.display()
        ),
    )
    .unwrap();

    skill_eval()
        .arg("teardown-guard")
        .current_dir(tmp.path())
        .assert()
        .success()
        .stdout(contains("Write guard removed"));

    assert!(!plugin.exists(), "the staged plugin is removed");
    assert!(!marker.exists(), "the marker is swept");
    assert!(
        !plugins.exists(),
        "the plugins dir created for the plugin alone is pruned"
    );
}

/// Like `guard`, the generic entry point must stay off layered descriptor
/// discovery: it fires per tool call and reads embedded descriptors only.
#[test]
fn guard_hook_generic_skips_descriptor_discovery() {
    let tmp = TempDir::new().unwrap();
    let marker = write_armed_marker(tmp.path(), &tmp.path().join(".eval-magic"));
    let harnesses = tmp.path().join(".eval-magic").join("harnesses");
    fs::create_dir_all(&harnesses).unwrap();
    fs::write(harnesses.join("broken.toml"), "label = ").unwrap();

    skill_eval()
        .args(["guard-hook", "--harness", "claude-code"])
        .arg(&marker)
        .current_dir(tmp.path())
        .write_stdin(r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#)
        .assert()
        .success()
        .stdout(contains(r#""permissionDecision":"deny""#))
        .stderr(contains("skipping harness descriptor").not());
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
    write_armed_marker(tmp.path(), &tmp.path().join(".eval-magic"));

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

/// The guard hook fires on every PreToolUse in a dispatched session, so it
/// must not run layered descriptor discovery: a broken user descriptor in the
/// eval env must not spam a skip-warning (or any other noise) per tool call.
#[test]
fn guard_hook_skips_descriptor_discovery() {
    let tmp = TempDir::new().unwrap();
    let marker = write_armed_marker(tmp.path(), &tmp.path().join(".eval-magic"));
    let harnesses = tmp.path().join(".eval-magic").join("harnesses");
    fs::create_dir_all(&harnesses).unwrap();
    fs::write(harnesses.join("broken.toml"), "label = ").unwrap();

    skill_eval()
        .arg("guard")
        .arg(&marker)
        .current_dir(tmp.path())
        .write_stdin(r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#)
        .assert()
        .success()
        .stdout(contains(r#""permissionDecision":"deny""#))
        .stderr(contains("skipping harness descriptor").not());
}
