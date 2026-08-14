//! Shared helpers for the `cli` integration tests.

use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Build a `Command` for the built `eval-magic` binary.
pub fn skill_eval() -> Command {
    let mut cmd = Command::cargo_bin("eval-magic").expect("binary `eval-magic` should build");
    // Disable user-global descriptor discovery so a developer's
    // ~/.config/eval-magic/harnesses never leaks into the tests.
    cmd.env("EVAL_MAGIC_CONFIG_DIR", "");
    cmd
}

/// A canonicalized temp root (resolves macOS /var → /private/var so the binary's
/// cwd-derived workspace path matches the fixtures it reads).
pub fn canonical_root() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    // `canonicalize` returns a verbatim (`\\?\`) path on Windows, but a child
    // process launched with it reports the plain form as its cwd — so paths the
    // CLI emits would never match ones a test joins onto this root.
    let root = match root.to_string_lossy().strip_prefix(r"\\?\") {
        Some(plain) => PathBuf::from(plain),
        None => root,
    };
    (tmp, root)
}
