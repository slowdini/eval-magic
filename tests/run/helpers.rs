//! Shared fixtures and helpers for the `run` integration tests.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;

pub const STAGED_MANIFEST: &str = ".slow-powers-eval-manifest.json";
pub const DEFAULT_EVALS: &str = r#"{ "skill_name": "mr-review", "evals": [ { "id": "e1", "prompt": "review this MR", "expected_output": "a review" } ] }"#;

pub fn skill_eval() -> Command {
    let mut cmd = Command::cargo_bin("eval-magic").expect("binary `eval-magic` should build");
    // Disable user-global descriptor discovery so a developer's
    // ~/.config/eval-magic/harnesses never leaks into the tests.
    cmd.env("EVAL_MAGIC_CONFIG_DIR", "");
    // Pin the eval home to the cwd the test runs in. The real default is a
    // per-skill directory under the user's data dir; a test must never write
    // there, and a relative value resolves against the cwd exactly as
    // `--workspace-dir` does. Tests that assert the real default clear this.
    cmd.env("EVAL_MAGIC_WORKSPACE_DIR", ".eval-magic");
    cmd
}

/// Build `<root>/skill-dir/mr-review/{SKILL.md,evals/evals.json}` and a `work`
/// cwd; returns `(skill_dir, cwd)`.
pub fn setup(root: &Path, evals_json: &str) -> (PathBuf, PathBuf) {
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    fs::write(
        skill_sub.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review merge requests\n---\n\nbody\n",
    )
    .unwrap();
    fs::write(skill_sub.join("evals").join("evals.json"), evals_json).unwrap();
    let cwd = root.join("work");
    fs::create_dir_all(&cwd).unwrap();
    (skill_dir, cwd)
}

pub fn iteration_dir(cwd: &Path) -> PathBuf {
    cwd.join(".eval-magic")
        .join("mr-review")
        .join("iteration-1")
}

/// A per-`(group, condition)` env dir — the cwd each `claude -p`/`codex exec`
/// subprocess runs from: `iteration-N/env-<group>-<condition>/`. Each holds only
/// that condition's skill (or none, for the control arm) and its group's fixtures.
/// Staging, fixtures, and the guard marker all land under here, below
/// `iteration_dir`; `RUNBOOK.md` lives above it in `iteration_dir`.
pub fn cli_env_dir(cwd: &Path, group: &str, condition: &str) -> PathBuf {
    iteration_dir(cwd).join(format!("env-{group}-{condition}"))
}

/// Staged skill names under the default single-group `with_skill` env's harness
/// skills dir (`env-g1-with_skill/.claude/skills`), excluding the staging
/// manifest, sorted.
pub fn env_staged_entries(cwd: &Path) -> Vec<String> {
    staged_entries(&cli_env_dir(cwd, "g1", "with_skill").join(".claude/skills"))
}

/// A local path as the artifacts spell it: forward slashes on every host, since
/// generated artifacts are a wire format shared across platforms. Mirrors
/// `eval_magic::core::fs::artifact_path` for the integration tests, which build
/// their expectations with `Path::join`.
pub fn wire_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// A `__fixture` invocation as a `command_check` command line.
///
/// The grader hands the string to `sh -c`. Double-quoting the program path and
/// arguments preserves spaces and literal fixture values.
pub fn fixture(args: &[&str]) -> String {
    let mut command = format!("\"{}\" __fixture", env!("CARGO_BIN_EXE_eval-magic"));
    for arg in args {
        command.push_str(&format!(" \"{arg}\""));
    }
    command
}

/// Mirrors `eval_magic::core::fs::real_path`, which the CLI applies to its own
/// roots. A test that compares a path the CLI emitted against one it built from
/// `TempDir` has to resolve its side the same way. On macOS, for example, temp
/// directories under `/var` resolve to `/private/var/...`.
pub fn resolved(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

/// The ref a task environment carries at the state the agent started from.
/// Mirrors `eval_magic::core::BASELINE_REF` for the integration tests, which
/// observe the environment through git rather than through the library.
pub const BASELINE_REF: &str = "refs/eval-magic/baseline";

/// Ask git about a task environment, as an operator inspecting one would.
/// Panics with git's own diagnostic, so a broken environment names itself.
pub fn git_stdout(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("git {} could not start: {error}", args.join(" ")));
    assert!(
        output.status.success(),
        "git {} failed in {}: {}",
        args.join(" "),
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// Every path under `root`, directories included. For asserting that
/// something is absent from an artifact tree, where a targeted `exists()` check
/// would only cover the one place it was expected.
pub fn walk_paths(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk_paths(&path));
        }
        found.push(path);
    }
    found
}

pub fn read_str(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// Names directly under `.claude/skills` (or `.agents/skills`), excluding
/// dotfiles (the staging manifest and the guard marker), sorted.
pub fn staged_entries(skills_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(skills_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names
}
