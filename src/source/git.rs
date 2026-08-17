//! Running git with the operator's configuration held off.
//!
//! Sourcing a codebase runs git against a URL from an eval config, on a host
//! whose git configuration belongs to someone else. Left inherited, that
//! configuration decides things the runner has to decide itself: `insteadOf`
//! rewrites the URL, so the tree sourced is not the tree the report cites;
//! `init.templateDir` installs hooks into a repository the guard assumes has
//! none; `commit.gpgSign` blocks the baseline commit on a passphrase prompt.
//!
//! So every git invocation in this module runs with system and global
//! configuration switched off and the environment-variable configuration
//! mechanism cleared.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::{GitOutput, clear_git_environment};

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

    pub(crate) fn run(&self, cwd: &Path, args: &[&str]) -> GitOutput {
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
