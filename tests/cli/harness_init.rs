//! `harness init`: scaffolding the commented descriptor template and notes
//! skeleton for a new harness — overwrite/overlay/stdout behavior included.

use crate::helpers::skill_eval;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn descriptor_path(root: &Path) -> PathBuf {
    root.join(".eval-magic/harnesses/cool-cli.toml")
}

fn notes_path(root: &Path) -> PathBuf {
    root.join(".eval-magic/harnesses/cool-cli-notes.md")
}

#[test]
fn harness_init_scaffolds_a_lint_clean_descriptor_and_notes_skeleton() {
    let tmp = TempDir::new().unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "init", "cool-cli"])
        .assert()
        .success()
        .stdout(
            contains("Scaffolded")
                .and(contains("✓ TOML syntax + schema"))
                .and(contains("all checks passed"))
                .and(contains("Next:")),
        );
    assert!(descriptor_path(tmp.path()).is_file());
    assert!(notes_path(tmp.path()).is_file());

    // The scaffold is lint-clean as written — via the real lint command, not
    // just the in-process validation `init` runs itself.
    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint", ".eval-magic/harnesses/cool-cli.toml"])
        .assert()
        .success()
        .stdout(contains("all checks passed"));
}

#[test]
fn harness_init_scaffold_registers_at_baseline() {
    let tmp = TempDir::new().unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "init", "cool-cli"])
        .assert()
        .success();
    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "list"])
        .assert()
        .success()
        .stdout(
            contains("cool-cli")
                .and(contains("project"))
                .and(contains("baseline")),
        );
}

#[test]
fn harness_init_refuses_to_overwrite_an_existing_descriptor() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".eval-magic/harnesses")).unwrap();
    fs::write(descriptor_path(tmp.path()), "label = \"cool-cli\"\n").unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "init", "cool-cli"])
        .assert()
        .failure()
        .stderr(contains("already exists").and(contains("--force")));

    let survived = fs::read_to_string(descriptor_path(tmp.path())).unwrap();
    assert_eq!(survived, "label = \"cool-cli\"\n");
    // Nothing is half-written: the notes skeleton is withheld too.
    assert!(!notes_path(tmp.path()).exists());
}

#[test]
fn harness_init_refuses_to_overwrite_an_existing_notes_file() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".eval-magic/harnesses")).unwrap();
    fs::write(notes_path(tmp.path()), "my precious notes\n").unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "init", "cool-cli"])
        .assert()
        .failure()
        .stderr(contains("already exists").and(contains("--force")));

    let survived = fs::read_to_string(notes_path(tmp.path())).unwrap();
    assert_eq!(survived, "my precious notes\n");
    assert!(!descriptor_path(tmp.path()).exists());
}

#[test]
fn harness_init_force_overwrites_both_files() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".eval-magic/harnesses")).unwrap();
    fs::write(descriptor_path(tmp.path()), "label = \"stale\"\n").unwrap();
    fs::write(notes_path(tmp.path()), "stale notes\n").unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "init", "cool-cli", "--force"])
        .assert()
        .success();

    let descriptor = fs::read_to_string(descriptor_path(tmp.path())).unwrap();
    assert!(descriptor.contains("label = \"cool-cli\""));
    let notes = fs::read_to_string(notes_path(tmp.path())).unwrap();
    assert!(!notes.contains("stale notes"));
}

#[test]
fn harness_init_stdout_prints_the_template_without_writing() {
    let tmp = TempDir::new().unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "init", "cool-cli", "--stdout"])
        .assert()
        .success()
        .stdout(contains("label = \"cool-cli\"").and(contains("Scaffolded").not()));

    assert!(!tmp.path().join(".eval-magic").exists());
}

#[test]
fn harness_init_rejects_a_non_kebab_case_name() {
    let tmp = TempDir::new().unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "init", "Not_Kebab"])
        .assert()
        .failure()
        .stderr(contains("kebab-case"));

    assert!(!tmp.path().join(".eval-magic").exists());
}

#[test]
fn harness_init_notes_overlay_semantics_for_registered_names() {
    let tmp = TempDir::new().unwrap();

    // Scaffolding a built-in's name is legitimate layering, not an error —
    // but the note must say the file overlays the registered harness.
    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "init", "claude-code"])
        .assert()
        .success()
        .stderr(contains("already registered").and(contains("overlay")));

    // The label-only scaffold merges onto the built-in as a no-op.
    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint", ".eval-magic/harnesses/claude-code.toml"])
        .assert()
        .success()
        .stdout(contains("all checks passed"));
}

#[test]
fn harness_init_template_prompts_for_verified_values() {
    let tmp = TempDir::new().unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "init", "cool-cli"])
        .assert()
        .success();

    let descriptor = fs::read_to_string(descriptor_path(tmp.path())).unwrap();
    for needle in [
        "VERIFY",
        "guess",
        "cool-cli-notes.md",
        "claude-stream-json",
        "codex-items",
        "[guard]",
        "harness lint",
    ] {
        assert!(
            descriptor.contains(needle),
            "descriptor template should mention {needle:?}"
        );
    }

    let notes = fs::read_to_string(notes_path(tmp.path())).unwrap();
    for needle in ["cool-cli", "Verified against", "Verification record"] {
        assert!(
            notes.contains(needle),
            "notes skeleton should mention {needle:?}"
        );
    }
}

/// The scaffolded template lands in user projects, so its authoring-guide
/// pointers name the embedded docs topic — not a repo-relative path a
/// binary-only user cannot open.
#[test]
fn harness_init_template_points_at_embedded_docs() {
    let tmp = TempDir::new().unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "init", "cool-cli"])
        .assert()
        .success();

    let descriptor = fs::read_to_string(descriptor_path(tmp.path())).unwrap();
    assert!(
        descriptor.contains("eval-magic docs byoh"),
        "descriptor template should point at the embedded docs topic"
    );
    assert!(
        !descriptor.contains("docs/byoh.md"),
        "descriptor template must not point at the repo-relative byoh path"
    );
}

#[test]
fn harness_init_help_documents_the_scaffold_workflow() {
    skill_eval()
        .args(["harness", "init", "--help"])
        .assert()
        .success()
        .stdout(
            contains("lint-clean")
                .and(contains("overlay"))
                .and(contains("eval-magic docs byoh"))
                .and(contains("--force"))
                .and(contains("guess")),
        );
}
