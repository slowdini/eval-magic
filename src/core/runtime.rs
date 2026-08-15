//! Runtime helpers.
//!
//! The synchronous git invocation helper. No runtime asset locator lives here:
//! the schemas are bundled into the binary at compile time (`include_str!`),
//! `clap` owns argument parsing, and the `error: <msg>` + exit(1) contract
//! lives in `src/main.rs`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

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

/// eval-magic's declared host requirement, stated once and reused by every
/// surface that can carry Markdown: the shell-discovery errors below, the `run`
/// preflight warnings, and the generated `RUNBOOK.md` and `dispatch-manifest.md`.
/// `--help` restates it in `cli::help::AFTER_HELP` instead, hard-wrapped and
/// without backticks, because clap renders into a terminal rather than Markdown.
///
/// It names `jq` as well as the shell deliberately. Harness `exec_template`s ship
/// as POSIX command lines (`</dev/null`, `&&`, `\` continuations), and the
/// parallel-dispatch and judge recipes are `jq` pipelines on top of them — Git
/// for Windows supplies `sh`, `xargs`, `tr`, and `wc` but not `jq`, so guidance
/// naming only the shell would send an operator to a setup that still walls out
/// at the judge step.
pub(crate) const POSIX_TOOLING_REQUIREMENT: &str = "eval-magic's dispatch and judge recipes are POSIX command lines built on `jq`, `xargs`, \
     `tr`, and `wc`. Run them in a POSIX shell with `jq` installed — on Windows, use Git Bash \
     (Git for Windows) or WSL. Set EVAL_MAGIC_SH to select a specific `sh`.";

/// `sh` locations inside a Git for Windows install, given the `git --exec-path`
/// directory (`<root>/mingw64/libexec/git-core` — hence three levels up).
/// Always spelled `sh.exe`: this layout only exists on Windows, and pinning the
/// name keeps the function testable on every host.
fn git_shell_candidates(exec_path: &Path) -> Vec<PathBuf> {
    let Some(root) = exec_path.ancestors().nth(3) else {
        return Vec::new();
    };
    if root.as_os_str().is_empty() {
        return Vec::new();
    }
    vec![
        root.join("bin").join("sh.exe"),
        root.join("usr").join("bin").join("sh.exe"),
    ]
}

/// First executable named `name` on `PATH`, or `None`.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let file_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(&file_name))
        .find(|candidate| candidate.is_file())
}

/// Where to look for `sh` on Windows once `PATH` has come up empty. Git for
/// Windows bundles one, but its default installer only puts `Git\cmd` on
/// `PATH`, so the shell has to be located through the install root instead.
fn windows_shell_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let git = run_git(&["--exec-path"], Path::new("."));
    if git.status == Some(0) {
        let exec_path = String::from_utf8_lossy(&git.stdout).trim().to_string();
        if !exec_path.is_empty() {
            candidates.extend(git_shell_candidates(Path::new(&exec_path)));
        }
    }
    candidates.push(PathBuf::from(r"C:\Program Files\Git\bin\sh.exe"));
    candidates.push(PathBuf::from(r"C:\Program Files\Git\usr\bin\sh.exe"));
    candidates
}

/// Locate a POSIX shell. `override_path` carries the operator's `EVAL_MAGIC_SH`
/// value and is passed in rather than read here so tests can exercise it
/// without mutating process environment.
///
/// Only ever searches for `sh`. On Windows `C:\Windows\System32\bash.exe` is
/// the WSL launcher, which resolves a different filesystem namespace — every
/// Windows path handed to it would name the wrong file.
fn discover_posix_shell(override_path: Option<&OsStr>) -> Result<PathBuf, String> {
    if let Some(value) = override_path {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
        // A configured-but-wrong override is an error, not a fallback: silently
        // discovering a different shell would hide the misconfiguration.
        return Err(format!(
            "EVAL_MAGIC_SH points at {}, which is not a file. {POSIX_TOOLING_REQUIREMENT}",
            path.display()
        ));
    }
    if let Some(found) = find_on_path("sh") {
        return Ok(found);
    }
    if cfg!(windows)
        && let Some(found) = windows_shell_candidates()
            .into_iter()
            .find(|candidate| candidate.is_file())
    {
        return Ok(found);
    }
    let posix_default = PathBuf::from("/bin/sh");
    if posix_default.is_file() {
        return Ok(posix_default);
    }
    Err(format!("no POSIX shell found. {POSIX_TOOLING_REQUIREMENT}"))
}

/// The resolved POSIX shell, discovered once per process. Callers spawn this
/// instead of a hardcoded `/bin/sh`, which does not exist on Windows.
pub(crate) fn posix_shell() -> Result<&'static Path, &'static str> {
    static SHELL: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    match SHELL.get_or_init(|| discover_posix_shell(std::env::var_os("EVAL_MAGIC_SH").as_deref())) {
        Ok(shell) => Ok(shell.as_path()),
        Err(message) => Err(message.as_str()),
    }
}

/// Announce that `test` is being skipped, `reason` explaining what the host
/// lacks. Returns `true` so a caller can `return` on it.
///
/// A skipped test still reports as passing, so the skip has to be impossible to
/// lose track of: setting `EVAL_MAGIC_REQUIRE_POSIX_TOOLS` (CI does) turns every
/// skip into a failure. One place owns that policy so each capability check does
/// not re-decide it.
#[cfg(test)]
pub(crate) fn report_skip(test: &str, reason: &str) -> bool {
    assert!(
        std::env::var_os("EVAL_MAGIC_REQUIRE_POSIX_TOOLS").is_none(),
        "{test} was skipped for a missing capability: {reason}. \
         Provide it, or unset EVAL_MAGIC_REQUIRE_POSIX_TOOLS to allow skipping"
    );
    eprintln!("skipping {test}: {reason}");
    true
}

/// The toolchain a shipped recipe shells out to: the parallel-dispatch and judge
/// recipes are `jq` pipelines over `xargs`, `tr`, and `wc`. Both the `run`
/// preflight that warns about a gap and the tests that execute a rendered recipe
/// check this same list, so neither can drift from what the recipes actually use.
pub(crate) const POSIX_RECIPE_TOOLS: &[&str] = &["jq", "xargs", "tr", "wc"];

/// The resolved shell, once every tool in `tools` is reachable from inside it,
/// otherwise an error naming the first one that is not.
///
/// The shipped parallel and judge recipes are POSIX pipelines over `jq`,
/// `xargs`, `tr`, and `wc`, so both a test that executes one and the `run`
/// preflight that warns about one need all of them. They are checked through the
/// shell rather than on the host `PATH` because that is where the recipe will
/// look: Git for Windows carries its own `/usr/bin`.
pub(crate) fn require_posix_toolchain(tools: &[&str]) -> Result<&'static Path, String> {
    let shell = posix_shell().map_err(str::to_string)?;
    for tool in tools {
        let found = Command::new(shell)
            .arg("-c")
            .arg(format!("command -v {tool}"))
            .output()
            .is_ok_and(|output| output.status.success());
        if !found {
            return Err(format!("{tool} is not on the PATH of {}", shell.display()));
        }
    }
    Ok(shell)
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
        assert!(res.stdout.is_empty());
        // Deliberately not matched against an errno spelling: the OS wording
        // differs per platform ("No such file or directory" vs "The system
        // cannot find the path specified"), and the contract is a readable
        // reason, not any particular one.
        let reason = String::from_utf8_lossy(&res.stderr);
        assert!(
            !reason.trim().is_empty(),
            "spawn failure reported no reason"
        );
    }

    /// The Git for Windows layout: `git --exec-path` points at
    /// `<root>/mingw64/libexec/git-core`, so the shell sits three levels up.
    #[test]
    fn git_shell_candidates_walk_up_from_the_git_core_exec_path() {
        let root = Path::new("/opt/Git");
        assert_eq!(
            git_shell_candidates(&root.join("mingw64").join("libexec").join("git-core")),
            vec![
                root.join("bin").join("sh.exe"),
                root.join("usr").join("bin").join("sh.exe"),
            ]
        );
    }

    /// An exec path too shallow to contain a Git root yields no candidates
    /// rather than walking off the top into `/`.
    #[test]
    fn git_shell_candidates_are_empty_for_a_rootless_exec_path() {
        assert!(git_shell_candidates(Path::new("git-core")).is_empty());
    }

    #[test]
    fn discover_posix_shell_accepts_an_explicit_override() {
        let existing = std::env::current_exe().unwrap();
        assert_eq!(
            discover_posix_shell(Some(existing.as_os_str())).unwrap(),
            existing
        );
    }

    /// A configured-but-wrong override is an error, never a silent fallback to
    /// discovery: the operator asked for a specific shell and needs to be told
    /// it is not there.
    #[test]
    fn discover_posix_shell_rejects_a_missing_override_with_setup_guidance() {
        let error = discover_posix_shell(Some("/nonexistent-shell-for-tests".as_ref()))
            .expect_err("a missing override should not fall through to discovery");
        assert!(error.contains("EVAL_MAGIC_SH"), "{error}");
        assert!(error.contains("/nonexistent-shell-for-tests"), "{error}");
        assert!(error.contains("Git Bash"), "{error}");
        assert!(error.contains("WSL"), "{error}");
        // The declared requirement is a POSIX shell *and* `jq`: Git for Windows
        // supplies the shell but not `jq`, so naming only the shell would send
        // an operator to a setup that still cannot run the judge recipe.
        assert!(error.contains("jq"), "{error}");
    }

    /// A POSIX shell is a declared development requirement, not a capability the
    /// suite probes for: the scripted-turn tests spawn a `#!/bin/sh` harness stub
    /// through the resolved shell and cannot skip. Asserting discovery outright
    /// makes that requirement fail here — one line, naming the fix — instead of
    /// somewhere deeper in the run boundary.
    #[test]
    fn discover_posix_shell_finds_a_real_shell() {
        let shell = discover_posix_shell(None).unwrap_or_else(|error| {
            panic!("a POSIX shell is required to develop eval-magic: {error}")
        });
        assert!(shell.is_file(), "{} is not a file", shell.display());
    }

    #[test]
    fn require_posix_toolchain_names_the_tool_that_is_missing() {
        let error = require_posix_toolchain(&["eval-magic-not-a-real-tool"])
            .expect_err("an uninstalled tool should be reported");
        assert!(error.contains("eval-magic-not-a-real-tool"), "{error}");
    }

    /// With nothing to look for, the check reduces to locating the shell — which
    /// is required, so this succeeds wherever the suite is allowed to run.
    #[test]
    fn require_posix_toolchain_with_no_tools_reduces_to_finding_the_shell() {
        let shell = require_posix_toolchain(&[]).expect("the required POSIX shell resolves");
        assert!(shell.is_file());
    }

    #[test]
    fn report_skip_panics_only_when_coverage_is_enforced() {
        // The unenforced path is the one this suite runs under; the enforced
        // path is covered by CI setting the variable.
        assert!(
            std::env::var_os("EVAL_MAGIC_REQUIRE_POSIX_TOOLS").is_some()
                || report_skip("demo", "a demo capability")
        );
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
