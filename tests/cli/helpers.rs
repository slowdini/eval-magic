//! Shared helpers for the `cli` integration tests.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Build a `Command` for the built `eval-magic` binary.
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

/// `fs::canonicalize` with Windows' verbatim (`\\?\`) prefix removed — the
/// spelling the CLI itself resolves paths to, and the one a child process
/// reports as its cwd. Fixtures built on any other spelling of the same
/// directory will not match the paths the CLI emits.
///
/// Both halves matter, and each is a different host's problem: the resolution
/// covers macOS (/var → /private/var), the stripping covers Windows.
pub fn resolved(path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(path).unwrap();
    match canonical.to_string_lossy().strip_prefix(r"\\?\") {
        Some(plain) => PathBuf::from(plain),
        None => canonical,
    }
}

/// A temp root already in the spelling [`resolved`] describes.
pub fn canonical_root() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = resolved(tmp.path());
    (tmp, root)
}
