//! Shared helpers for the `cli` integration tests.

use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Build a `Command` for the built `skill-eval` binary.
pub fn skill_eval() -> Command {
    Command::cargo_bin("skill-eval").expect("binary `skill-eval` should build")
}

/// A canonicalized temp root (resolves macOS /var → /private/var so the binary's
/// cwd-derived workspace path matches the fixtures it reads).
pub fn canonical_root() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    (tmp, root)
}
