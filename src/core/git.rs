//! Running git with the operator's configuration held off.
//!
//! The runner spawns git on a host whose git configuration belongs to someone
//! else. Left inherited, that configuration decides things the runner has to
//! decide itself: `insteadOf` rewrites a URL, so the tree sourced is not the
//! tree the report cites; `init.templateDir` installs hooks into a repository
//! the guard assumes has none; `commit.gpgSign` blocks the baseline commit on a
//! passphrase prompt; `core.excludesFile` and `core.autocrlf` change which files
//! a diff reports and how many lines it counts.
//!
//! So every caller that needs an answer git alone should decide runs through
//! [`IsolatedGit`]: system and global configuration switched off, and the
//! environment-variable configuration mechanism cleared.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::{GitOutput, clear_git_environment};

/// Marks the state every task environment starts from.
///
/// The runner writes it once, when it establishes the environment's
/// repository; every later measurement is the difference from it. Deliberately
/// outside `refs/heads/`: it never appears in `git branch`, so it adds nothing
/// to what the agent under test sees.
pub const BASELINE_REF: &str = "refs/eval-magic/baseline";

/// A scratch git configuration that resolves to nothing.
///
/// Holds the `TempDir` alive: dropping it removes the empty global config file
/// and the empty template directory that make the isolation work.
pub(crate) struct IsolatedGit {
    _scratch: tempfile::TempDir,
    global_config: PathBuf,
    template_dir: PathBuf,
}

impl IsolatedGit {
    pub(crate) fn new() -> Result<Self, String> {
        let scratch = tempfile::TempDir::new()
            .map_err(|error| format!("could not create isolated Git configuration: {error}"))?;
        let global_config = scratch.path().join("global-config");
        let template_dir = scratch.path().join("template");
        std::fs::write(&global_config, "")
            .map_err(|error| format!("could not create empty Git configuration: {error}"))?;
        std::fs::create_dir(&template_dir)
            .map_err(|error| format!("could not create empty Git template directory: {error}"))?;
        Ok(Self {
            _scratch: scratch,
            global_config,
            template_dir,
        })
    }

    /// An empty template directory, for `git init --template`, so a configured
    /// `init.templateDir` cannot seed hooks into a task repository.
    pub(crate) fn template_dir(&self) -> &Path {
        &self.template_dir
    }

    /// Invoke git in `cwd`. `env` sets variables for this invocation only —
    /// the committer identity a deterministic baseline commit needs, or the
    /// scratch index a measurement builds; configuration still comes from the
    /// isolated files above.
    ///
    /// `env` is applied *after* the routing variables are cleared, deliberately:
    /// `GIT_INDEX_FILE` is one of the variables cleared, so a caller pointing
    /// git at an index of its own has to win over the inherited state rather
    /// than be swept up with it.
    pub(crate) fn run(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> GitOutput {
        let mut command = Command::new("git");
        command
            // `git clone` and `git init` create paths inside `.git` before any
            // repository-local configuration exists, so the Windows long-path
            // lift has to ride on the invocation itself.
            .args(["-c", "core.longpaths=true"])
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.global_config)
            // The environment-variable configuration mechanism: git reads
            // `GIT_CONFIG_KEY_<n>` / `GIT_CONFIG_VALUE_<n>` only up to the count,
            // so clearing the count disables all of them.
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS");
        clear_git_environment(&mut command);
        for (name, value) in env {
            command.env(name, value);
        }
        match command.output() {
            Ok(output) => GitOutput {
                status: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
            },
            Err(error) => GitOutput {
                status: None,
                stdout: Vec::new(),
                stderr: format!("{error}").into_bytes(),
            },
        }
    }
}
