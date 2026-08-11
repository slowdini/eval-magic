//! File-layer modes for `harness lint`, including the built-in developer opt-in.

use super::*;

#[test]
fn harness_lint_as_builtin_passes_embedded_descriptor_sources() {
    let tmp = TempDir::new().unwrap();
    let harnesses_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("harnesses");

    for name in ["claude-code", "cline", "codex", "opencode"] {
        let file = harnesses_dir.join(format!("{name}.toml"));
        skill_eval()
            .current_dir(tmp.path())
            .args(["harness", "lint", "--as-builtin"])
            .arg(&file)
            .assert()
            .success()
            .stdout(
                contains("built-in source mode")
                    .and(contains("user-layer restrictions skipped"))
                    .and(contains("does not change registry loading")),
            );
    }
}

#[test]
fn harness_lint_file_explains_the_default_user_layer_mode() {
    let tmp = TempDir::new().unwrap();
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("harnesses")
        .join("claude-code.toml");

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .assert()
        .failure()
        .stdout(
            contains("linting file as a user-supplied descriptor")
                .and(contains("--as-builtin"))
                .and(contains("harness lint <name>"))
                .and(contains("compiled-in descriptor")),
        );
}

#[test]
fn harness_lint_as_builtin_requires_a_file_target() {
    let tmp = TempDir::new().unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint", "claude-code", "--as-builtin"])
        .assert()
        .failure()
        .stderr(
            contains("--as-builtin requires a descriptor file path").and(contains(
                "registered harness names already preserve source layers",
            )),
        );
}

#[test]
fn harness_lint_as_builtin_conflicts_with_harness_file() {
    let tmp = TempDir::new().unwrap();
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("harnesses")
        .join("claude-code.toml");

    skill_eval()
        .current_dir(tmp.path())
        .arg("--harness-file")
        .arg(&file)
        .args(["harness", "lint", "--as-builtin"])
        .arg(&file)
        .assert()
        .failure()
        .stderr(
            contains("--as-builtin")
                .and(contains("--harness-file"))
                .and(contains("cannot be used with")),
        );
}

#[test]
fn harness_lint_help_documents_file_modes_and_probe_flags() {
    skill_eval()
        .args(["harness", "lint", "--help"])
        .assert()
        .success()
        .stdout(
            contains("file targets are linted as user-supplied by default")
                .and(contains("--as-builtin"))
                .and(contains("does not change registry loading"))
                .and(contains("cannot be combined"))
                .and(contains("--harness-file"))
                .and(contains("--probe"))
                .and(contains("--yes"))
                .and(contains("--probe-timeout")),
        );
}
