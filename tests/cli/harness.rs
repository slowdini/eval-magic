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
fn harness_list_names_layers_and_enhancements() {
    let tmp = TempDir::new().unwrap();
    write_project_descriptor(tmp.path(), "cool.toml", "label = \"cool-custom-harness\"\n");

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "list"])
        .assert()
        .success()
        .stdout(
            contains("claude-code")
                .and(contains("(default)"))
                .and(contains("built-in"))
                .and(contains("opencode"))
                .and(contains("cool-custom-harness"))
                .and(contains("project"))
                .and(contains("baseline"))
                .and(contains("transcript")),
        );
}

#[test]
fn harness_list_marks_merged_builtins() {
    let tmp = TempDir::new().unwrap();
    write_project_descriptor(
        tmp.path(),
        "claude-code.toml",
        "label = \"claude-code\"\n\n[model]\nflag = \"--model-x\"\n",
    );

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "list"])
        .assert()
        .success()
        .stdout(contains("built-in + project"));
}

#[test]
fn harness_show_prints_the_merged_descriptor_as_toml() {
    let tmp = TempDir::new().unwrap();
    write_project_descriptor(
        tmp.path(),
        "claude-code.toml",
        "label = \"claude-code\"\n\n[model]\nflag = \"--model-x\"\n",
    );

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "show", "claude-code"])
        .assert()
        .success()
        .stdout(
            contains("label = \"claude-code\"")
                .and(contains("--model-x"))
                .and(contains("harnesses/claude-code.toml (built-in)"))
                .and(contains("claude-code.toml (project)"))
                // The rest of the embedded descriptor survives the merge.
                .and(contains("skills_dir = \".claude/skills\"")),
        );
}

#[test]
fn harness_show_unknown_name_lists_known_harnesses() {
    let tmp = TempDir::new().unwrap();
    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "show", "nonexistent"])
        .assert()
        .failure()
        .stderr(contains("unknown harness 'nonexistent'").and(contains("claude-code")));
}

#[test]
fn harness_lint_passes_a_valid_file() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("cool.toml");
    fs::write(&file, "label = \"cool-custom-harness\"\n").unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .assert()
        .success()
        .stdout(contains("✓"));
}

#[test]
fn harness_lint_reports_schema_violations() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("bad.toml");
    fs::write(&file, "label = \"bad\"\nmystery = 1\n").unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .assert()
        .failure()
        .stderr(contains("✗").and(contains("mystery")));
}

#[test]
fn harness_lint_rejects_a_user_guard_block() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("armed.toml");
    fs::write(
        &file,
        r#"
label = "armed"

[guard]
hooks_file = ".armed/hooks.json"
matcher = "Write"
command_template = '"{exe}" guard-hook --harness armed "{marker}"'
hook_entry = '{"matcher":"{matcher}","hooks":[{"type":"command","command":"{command}"}]}'
verdict_template = '{"decision":"block","reason":"{reason}"}'
armed_message = "x"
"#,
    )
    .unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .assert()
        .failure()
        .stderr(contains("may not declare [guard]").and(contains("detect-stray-writes")));
}

#[test]
fn harness_lint_reports_invariant_breaks() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("slug.toml");
    fs::write(
        &file,
        "label = \"demo\"\nskills_dir = \".demo/skills\"\nconfig_dirs = [\".demo\"]\n\n\
         [staging]\nslug_template = \"{prefix}{iteration}-{condition}\"\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .assert()
        .failure()
        .stderr(contains("staging.slug_template").and(contains("{skill_name}")));
}

#[test]
fn harness_lint_checks_a_partial_override_against_its_merge_target() {
    let tmp = TempDir::new().unwrap();
    // Standalone this partial is fine; merged onto claude-code it orphans
    // skills_dir's parent — lint must catch the post-merge break.
    let file = tmp.path().join("claude-code.toml");
    fs::write(
        &file,
        "label = \"claude-code\"\nconfig_dirs = [\".other\"]\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .assert()
        .failure()
        .stderr(contains("parent of skills_dir"));
}

#[test]
fn harness_lint_accepts_a_registered_harness_name() {
    let tmp = TempDir::new().unwrap();
    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint", "claude-code"])
        .assert()
        .success()
        .stdout(contains("✓"));
}

#[test]
fn harness_lint_of_a_name_reports_the_skipped_project_file() {
    let tmp = TempDir::new().unwrap();
    write_project_descriptor(
        tmp.path(),
        "broken.toml",
        "label = \"broken\"\nmystery = 1\n",
    );

    // Init skips the broken file with a warning; linting the name surfaces
    // the full per-file report and fails.
    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint", "broken"])
        .assert()
        .failure()
        .stderr(contains("✗").and(contains("mystery")));
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
