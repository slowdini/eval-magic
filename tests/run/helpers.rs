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
/// The grader hands the string to the platform shell, so it has to parse the
/// same under `sh -c` and `cmd /C`: a double-quoted program path followed by
/// double-quoted arguments does. One such command covers what `test`, `true`,
/// and `fc` would each have to spell differently per shell.
pub fn fixture(args: &[&str]) -> String {
    let mut command = format!("\"{}\" __fixture", env!("CARGO_BIN_EXE_eval-magic"));
    for arg in args {
        command.push_str(&format!(" \"{arg}\""));
    }
    command
}

/// `fs::canonicalize` with Windows' verbatim (`\\?\`) prefix removed.
///
/// The symlink resolution matters — a macOS temp dir lives under a symlinked
/// `/var`, so the CLI's own paths resolve to `/private/var/...`. The prefix does
/// not: a child process reports the plain form as its cwd, so plain is the
/// spelling every path the CLI emits actually carries.
pub fn resolved(path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(path).unwrap();
    match canonical.to_string_lossy().strip_prefix(r"\\?\") {
        Some(plain) => PathBuf::from(plain),
        None => canonical,
    }
}

pub fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
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
