//! Layered harness-descriptor discovery: user-global and project-local
//! descriptor files, the one-off `--harness-file`, and the fail-soft policy
//! for broken discovered files.

use crate::helpers::skill_eval;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Write `<root>/.eval-magic/harnesses/<file>` with the given TOML.
fn write_project_descriptor(root: &Path, file: &str, contents: &str) {
    let dir = root.join(".eval-magic").join("harnesses");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(file), contents).unwrap();
}

#[test]
fn project_local_descriptor_registers_a_new_harness() {
    let tmp = TempDir::new().unwrap();
    write_project_descriptor(tmp.path(), "cool.toml", "label = \"cool-custom-harness\"\n");

    // The command still fails later (no skill context), but the harness name
    // resolves — the registry saw the project-local layer.
    skill_eval()
        .current_dir(tmp.path())
        .args(["aggregate", "--harness", "cool-custom-harness"])
        .assert()
        .failure()
        .stderr(contains("unknown harness").not());
}

#[test]
fn unknown_harness_error_lists_discovered_harnesses() {
    let tmp = TempDir::new().unwrap();
    write_project_descriptor(tmp.path(), "cool.toml", "label = \"cool-custom-harness\"\n");

    skill_eval()
        .current_dir(tmp.path())
        .args(["aggregate", "--harness", "nonexistent"])
        .assert()
        .failure()
        .stderr(
            contains("unknown harness 'nonexistent'")
                .and(contains("claude-code"))
                .and(contains("cool-custom-harness")),
        );
}

#[test]
fn user_global_layer_is_discovered_via_config_dir_env() {
    let tmp = TempDir::new().unwrap();
    let config_root = tmp.path().join("config");
    let dir = config_root.join("harnesses");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("cool.toml"), "label = \"cool-custom-harness\"\n").unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .env("EVAL_MAGIC_CONFIG_DIR", &config_root)
        .args(["aggregate", "--harness", "cool-custom-harness"])
        .assert()
        .failure()
        .stderr(contains("unknown harness").not());
}

#[test]
fn harness_file_registers_a_one_off_harness() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("cool.toml");
    fs::write(&file, "label = \"cool-custom-harness\"\n").unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .arg("aggregate")
        .arg("--harness-file")
        .arg(&file)
        .args(["--harness", "cool-custom-harness"])
        .assert()
        .failure()
        .stderr(contains("unknown harness").not());
}

#[test]
fn broken_project_local_descriptor_warns_but_never_bricks() {
    let tmp = TempDir::new().unwrap();
    write_project_descriptor(
        tmp.path(),
        "broken.toml",
        "label = \"broken\"\nmystery = 1\n",
    );

    // The built-in harness still resolves; the broken file is skipped with a
    // warning that points at the lint command.
    skill_eval()
        .current_dir(tmp.path())
        .args(["aggregate", "--harness", "claude-code"])
        .assert()
        .failure()
        .stderr(
            contains("skipping harness descriptor")
                .and(contains("harness lint"))
                .and(contains("unknown harness").not()),
        );
}

#[test]
fn broken_harness_file_is_fatal() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("broken.toml");
    fs::write(&file, "label = ").unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .arg("aggregate")
        .arg("--harness-file")
        .arg(&file)
        .assert()
        .failure()
        .stderr(contains("broken.toml").and(contains("invalid TOML")));
}

#[test]
fn missing_harness_file_is_fatal() {
    let tmp = TempDir::new().unwrap();
    skill_eval()
        .current_dir(tmp.path())
        .arg("aggregate")
        .arg("--harness-file")
        .arg(tmp.path().join("missing.toml"))
        .assert()
        .failure()
        .stderr(contains("--harness-file").and(contains("missing.toml")));
}
