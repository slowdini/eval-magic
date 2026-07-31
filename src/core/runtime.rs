//! Runtime helpers.
//!
//! The synchronous git invocation helper. No runtime asset locator lives here:
//! the schemas are bundled into the binary at compile time (`include_str!`),
//! `clap` owns argument parsing, and the `error: <msg>` + exit(1) contract
//! lives in `src/main.rs`.

use std::path::Path;
use std::process::Command;

/// Inherited Git routing variables that can redirect repository discovery or
/// object/index access away from a command's current working directory.
pub(crate) const GIT_ROUTING_ENV_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CEILING_DIRECTORIES",
];

/// Remove inherited repository-routing state before running a command whose
/// current directory is intended to define its Git boundary.
pub(crate) fn clear_git_environment(command: &mut Command) {
    for name in GIT_ROUTING_ENV_VARS {
        command.env_remove(name);
    }
}

/// Validate one environment override that must work in both a POSIX-shell
/// recipe and a directly spawned scripted round.
pub(crate) fn validate_agent_environment_entry(name: &str, value: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let portable_name = chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric());
    if !portable_name {
        return Err(format!(
            "agent environment variable name {name:?} must match [A-Za-z_][A-Za-z0-9_]*"
        ));
    }
    if GIT_ROUTING_ENV_VARS.contains(&name) {
        return Err(format!(
            "agent environment variable {name:?} is reserved so dispatches stay inside the task repository"
        ));
    }
    if value.contains('\0') {
        return Err(format!(
            "agent environment variable {name:?} value must not contain NUL"
        ));
    }
    Ok(())
}

/// Outcome of a git invocation.
///
/// `status` is `None` when git could not be spawned at all (e.g. ENOENT, a
/// nonexistent cwd, permission denied); the reason is surfaced into `stderr`,
/// so callers have one channel for both git's own errors and spawn failures.
/// `stdout` and
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
    let mut command = Command::new("git");
    command.args(args).current_dir(cwd);
    clear_git_environment(&mut command);
    match command.output() {
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
    /// is `None` and the spawn error is surfaced into stderr — the contract is
    /// the null status plus a readable reason, not any particular error-code
    /// spelling.
    #[test]
    fn run_git_spawn_error_surfaced() {
        let res = run_git(
            &["rev-parse", "HEAD"],
            "/nonexistent-dir-for-rungit-test".as_ref(),
        );
        assert_eq!(res.status, None);
        assert!(String::from_utf8_lossy(&res.stderr).contains("No such file or directory"));
    }

    #[test]
    fn agent_environment_validation_accepts_portable_names_and_empty_values() {
        assert!(validate_agent_environment_entry("TZ", "UTC").is_ok());
        assert!(validate_agent_environment_entry("_EMPTY", "").is_ok());
        assert!(validate_agent_environment_entry("MODE_2", "a=b").is_ok());
    }

    #[test]
    fn agent_environment_validation_rejects_unsafe_names_values_and_git_routing() {
        for name in ["", "9TZ", "BAD-NAME", "GIT_DIR", "GIT_WORK_TREE"] {
            let error = validate_agent_environment_entry(name, "value").unwrap_err();
            assert!(error.contains(name), "{name:?}: {error}");
        }
        let error = validate_agent_environment_entry("TZ", "bad\0value").unwrap_err();
        assert!(error.contains("NUL"), "{error}");
    }
}
