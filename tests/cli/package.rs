//! Packaging and release-channel metadata.

use std::path::Path;
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read_repo_file(path: &str) -> String {
    std::fs::read_to_string(repo_root().join(path)).unwrap_or_else(|err| {
        panic!("expected to read {path}: {err}");
    })
}

#[test]
fn source_files_advertise_crates_io_publish_channel() {
    let manifest = read_repo_file("Cargo.toml");
    assert!(manifest.contains(r#"homepage = "https://github.com/slowdini/eval-magic""#));
    assert!(manifest.contains(r#"readme = "README.md""#));
    assert!(manifest.contains(r#"keywords = ["evals", "agents", "skills"]"#));
    assert!(manifest.contains(r#"categories = ["command-line-utilities"]"#));
    assert!(manifest.contains(r#""/.github/**""#));
    assert!(manifest.contains(r#""/.cargo-husky/**""#));
    assert!(manifest.contains(r#""/.claude/**""#));
    assert!(manifest.contains(r#""AGENTS.md""#));
    assert!(manifest.contains(r#""CLAUDE.md""#));

    let readme = read_repo_file("README.md");
    assert!(readme.contains("cargo install eval-magic"));

    // crates.io publishing is intentionally NOT wired through cargo-dist: a dist
    // `publish-jobs` reusable workflow only receives the caller's id-token/packages
    // permissions, so its checkout can't get `contents: read` and the whole release
    // workflow fails at startup. Publishing lives in a standalone Trusted Publishing
    // workflow instead — guard against accidentally re-coupling.
    let dist_config = read_repo_file("dist-workspace.toml");
    assert!(!dist_config.contains("publish-jobs"));

    // Trusted Publishing: the version tag triggers an OIDC-authenticated
    // `cargo publish` (no long-lived CARGO_REGISTRY_TOKEN secret). The trigger is
    // the tag, NOT `release: published` — dist undrafts the release with the
    // default GITHUB_TOKEN, and GITHUB_TOKEN-triggered events never start new
    // workflow runs, so a `release` trigger would silently never fire.
    let workflow = read_repo_file(".github/workflows/publish-crates.yml");
    assert!(workflow.contains("tags:"));
    assert!(!workflow.contains("types: [published]"));
    assert!(workflow.contains("rust-lang/crates-io-auth-action"));
    assert!(workflow.contains("id-token: write"));
    assert!(workflow.contains("cargo publish --locked"));
}

#[test]
fn ci_publishes_default_branch_coverage_for_readme_badge() {
    let workflow = read_repo_file(".github/workflows/ci.yml");

    for expected in [
        "  push:\n    branches: [dev]",
        "if: github.event_name == 'push'",
        "id-token: write",
        "components: llvm-tools-preview",
        "taiki-e/install-action@cargo-llvm-cov",
        "cargo llvm-cov --all-targets --all-features --workspace --locked",
        "codecov/codecov-action@v5",
        "files: lcov.info",
        "use_oidc: true",
        "fail_ci_if_error: true",
    ] {
        assert!(workflow.contains(expected), "CI is missing {expected}");
    }
}

/// Releases attach a Windows binary, so the suite has to run on Windows — but
/// the matrix entry alone proves nothing. Six of those tests are gated on
/// capabilities the runner has to be handed: `jq` for the judge recipes, and
/// symlink creation for the `core::fs` round-trips. Without the enforcement
/// variable they skip in silence, and the job reports green while covering
/// strictly less than it looks like it is. Every string below is load-bearing,
/// which is why they are pinned together rather than one standing for the rest.
#[test]
fn ci_runs_the_suite_on_windows_with_capability_skips_enforced() {
    let workflow = read_repo_file(".github/workflows/ci.yml");

    for expected in [
        "os: [ubuntu-latest, windows-latest]",
        "fail-fast: false",
        "EVAL_MAGIC_REQUIRE_POSIX_TOOLS: 1",
        "choco install jq",
        "AllowDevelopmentWithoutDevLicense",
    ] {
        assert!(workflow.contains(expected), "CI is missing {expected}");
    }
}

#[test]
fn cargo_package_excludes_repo_local_authoring_files() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args(["package", "--list", "--allow-dirty"])
        .current_dir(repo_root())
        .output()
        .expect("cargo package --list should run");

    assert!(
        output.status.success(),
        "cargo package --list failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let files = String::from_utf8(output.stdout).expect("package list should be utf-8");
    for required in [
        "Cargo.lock",
        "Cargo.toml",
        "LICENSE",
        "README.md",
        "assets/readme.png",
        "schema/evals.schema.json",
        "schema/harness-descriptor.schema.json",
        "profiles/shared/plan-mode.md",
        "harnesses/claude-code.toml",
        "harnesses/cline.toml",
        "harnesses/codex.toml",
        "harnesses/opencode.toml",
        "harnesses/template.toml",
        "harnesses/template-notes.md",
        "src/main.rs",
    ] {
        assert!(
            files.lines().any(|line| line == required),
            "{required} should be packaged"
        );
    }

    // Every shipped guide is discovered by build.rs and embedded in the
    // binary, so packaging must follow the directory rather than a duplicated
    // topic list.
    let guide_dir = repo_root().join("docs/guides");
    for entry in std::fs::read_dir(&guide_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        // `cargo package --list` prints forward slashes on every platform.
        let relative = path
            .strip_prefix(repo_root())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert!(
            files.lines().any(|line| line == relative),
            "{relative} should be packaged"
        );
    }

    for excluded in [
        ".cargo-husky/hooks/pre-commit",
        ".claude/settings.json",
        ".github/workflows/ci.yml",
        "AGENTS.md",
        "CLAUDE.md",
    ] {
        assert!(
            files.lines().all(|line| line != excluded),
            "{excluded} should not be packaged"
        );
    }
}
