//! Runtime helpers.
//!
//! Ports the parts of `src/core/runtime.ts` that have a Rust equivalent. The
//! Bun-vs-Node portability shims (`argv`, `moduleDir`, `isMain`) and the
//! `die`/`CliError`/`runCli` error contract do not carry over: `clap` owns
//! argument parsing, and the `error: <msg>` + exit(1) contract already lives in
//! `src/main.rs`. `packageRoot` is deliberately omitted — the schemas it located
//! at runtime in the TS tree will be bundled into the binary at compile time
//! (Phase 2, `include_str!`), so no runtime asset locator is needed.
//!
//! What remains is the synchronous git invocation helper.

use std::path::Path;
use std::process::Command;

/// Outcome of a git invocation.
///
/// `status` is `None` when git could not be spawned at all (e.g. ENOENT, a
/// nonexistent cwd, permission denied); the reason is surfaced into `stderr`,
/// matching the TS contract where callers read git's own stderr. `stdout` and
/// `stderr` are raw bytes — callers that read file contents out of git
/// (`git show`) need the undecoded buffer, not a lossy UTF-8 string.
#[derive(Debug)]
pub struct GitOutput {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Synchronously invoke `git` with `args` in `cwd`, returning its status and raw
/// output. A failure to spawn git is not an error here: it yields `status: None`
/// with the spawn error surfaced into `stderr`, so callers can handle it
/// alongside git's own failures.
pub fn run_git(args: &[&str], cwd: &Path) -> GitOutput {
    match Command::new("git").args(args).current_dir(cwd).output() {
        Ok(out) => GitOutput {
            status: out.status.code(),
            stdout: out.stdout,
            stderr: out.stderr,
        },
        Err(err) => GitOutput {
            status: None,
            stdout: Vec::new(),
            stderr: format!("{err}").into_bytes(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A successful git command returns exit status 0 and writes to stdout.
    /// Run against this crate's own repo, which has commit history.
    #[test]
    fn run_git_success_status_and_stdout() {
        let res = run_git(
            &["rev-parse", "--short", "HEAD"],
            env!("CARGO_MANIFEST_DIR").as_ref(),
        );
        assert_eq!(res.status, Some(0));
        assert!(String::from_utf8_lossy(&res.stdout).trim().len() > 3);
    }

    /// A git command that fails (bad ref) returns a non-zero status.
    #[test]
    fn run_git_failing_command_nonzero() {
        let res = run_git(
            &["rev-parse", "not-a-real-ref-xyz"],
            env!("CARGO_MANIFEST_DIR").as_ref(),
        );
        assert_ne!(res.status, Some(0));
    }

    /// When git itself cannot be spawned (here, a nonexistent cwd), the status
    /// is `None` and the spawn error is surfaced into stderr. The TS original
    /// asserts the stderr contains "ENOENT"; Rust's `io::Error` Display reads
    /// "No such file or directory" instead, so we assert on that — the behavior
    /// (null status, spawn error in stderr) is what we're porting, not Node's
    /// error-code spelling.
    #[test]
    fn run_git_spawn_error_surfaced() {
        let res = run_git(
            &["rev-parse", "HEAD"],
            "/nonexistent-dir-for-rungit-test".as_ref(),
        );
        assert_eq!(res.status, None);
        assert!(String::from_utf8_lossy(&res.stderr).contains("No such file or directory"));
    }
}
