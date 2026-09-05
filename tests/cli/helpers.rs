//! Shared helpers for the `cli` integration tests.

use assert_cmd::Command;
use serde_json::{Value, json};
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

/// The canonical spelling the CLI resolves paths to.
///
/// Fixtures built on an alias of the same directory will not match paths the
/// CLI emits. This matters on macOS, where `/var` resolves to `/private/var`.
pub fn resolved(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

/// A temp root already in the spelling [`resolved`] describes.
pub fn canonical_root() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = resolved(tmp.path());
    (tmp, root)
}

/// Give a test-authored eval config the mandatory local codebase when its
/// subject is a later pipeline stage rather than environment provisioning.
pub fn with_default_codebase(evals: &Value) -> Value {
    let mut evals = evals.clone();
    if evals.get("codebase").is_none() {
        evals["codebase"] = json!({ "path": "." });
    }
    evals
}
