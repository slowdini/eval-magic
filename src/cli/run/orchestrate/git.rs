//! Runner-owned Git lifecycle for private task environments.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::core::{clear_git_environment, run_git};

use super::super::RunError;
use super::Resolved;
use super::envs::{EnvLayoutInput, env_targets};
use crate::core::RunContext;

const BASELINE_BRANCH: &str = "work";
const BASELINE_MESSAGE: &str = "eval-magic task baseline";
const BASELINE_NAME: &str = "eval-magic";
const BASELINE_EMAIL: &str = "eval-magic@localhost";
const BASELINE_DATE: &str = "2000-01-01T00:00:00Z";

pub(super) fn preflight_git(ctx: &RunContext) -> Result<(), RunError> {
    let output = run_git(&["--version"], &ctx.skill_subdir);
    if output.status == Some(0) {
        return Ok(());
    }
    Err(RunError::msg(format!(
        "Git is required to prepare isolated task repositories: {}",
        git_diagnostic(output.status, &output.stderr)
    )))
}

pub(super) fn initialize_task_repositories(resolved: &Resolved) -> Result<(), RunError> {
    let targets = env_targets(&EnvLayoutInput {
        iteration_dir: &resolved.iteration_dir,
        groups: &resolved.groups,
        cond_a: resolved.cond_a,
        cond_b: resolved.cond_b,
        skill_path_a: resolved.skill_path_a.as_deref(),
        skill_path_b: resolved.skill_path_b.as_deref(),
    });
    for target in targets {
        initialize_task_repository(&target.root).map_err(|error| {
            RunError::msg(format!(
                "could not initialize task Git repository at {}: {error}",
                target.root.display()
            ))
        })?;
    }
    Ok(())
}

fn initialize_task_repository(root: &Path) -> Result<(), String> {
    remove_existing_git_dir(root)?;

    let isolated = tempfile::TempDir::new()
        .map_err(|error| format!("could not create isolated Git configuration: {error}"))?;
    let template_dir = isolated.path().join("template");
    let global_config = isolated.path().join("global-config");
    fs::create_dir(&template_dir)
        .map_err(|error| format!("could not create empty Git template directory: {error}"))?;
    fs::write(&global_config, "")
        .map_err(|error| format!("could not create empty Git configuration: {error}"))?;

    run_checked(
        root,
        &[
            OsString::from("init"),
            OsString::from("--quiet"),
            OsString::from("--initial-branch"),
            OsString::from(BASELINE_BRANCH),
            OsString::from("--template"),
            template_dir.into_os_string(),
            OsString::from("."),
        ],
        &global_config,
        &[],
    )?;

    let hooks_dir = root.join(".git/eval-magic-disabled-hooks");
    fs::create_dir_all(root.join(".git/info"))
        .map_err(|error| format!("could not create Git info directory: {error}"))?;
    fs::create_dir_all(&hooks_dir)
        .map_err(|error| format!("could not create empty Git hooks directory: {error}"))?;
    fs::write(root.join(".git/info/exclude"), "/.eval-magic-outputs/\n")
        .map_err(|error| format!("could not configure framework output exclusion: {error}"))?;

    for (name, value) in [
        ("user.name", OsString::from(BASELINE_NAME)),
        ("user.email", OsString::from(BASELINE_EMAIL)),
        ("commit.gpgSign", OsString::from("false")),
        ("tag.gpgSign", OsString::from("false")),
        ("core.hooksPath", hooks_dir.into_os_string()),
    ] {
        run_checked(
            root,
            &[
                OsString::from("config"),
                OsString::from("--local"),
                OsString::from(name),
                value,
            ],
            &global_config,
            &[],
        )?;
    }

    run_checked(
        root,
        &[
            OsString::from("add"),
            OsString::from("--force"),
            OsString::from("--all"),
            OsString::from("--"),
            OsString::from("."),
            OsString::from(":(exclude,top).eval-magic-outputs"),
            OsString::from(":(exclude,top).eval-magic-outputs/**"),
        ],
        &global_config,
        &[],
    )?;
    run_checked(
        root,
        &[
            OsString::from("commit"),
            OsString::from("--quiet"),
            OsString::from("--allow-empty"),
            OsString::from("--no-gpg-sign"),
            OsString::from("--no-verify"),
            OsString::from("-m"),
            OsString::from(BASELINE_MESSAGE),
        ],
        &global_config,
        &[
            ("GIT_AUTHOR_NAME", BASELINE_NAME),
            ("GIT_AUTHOR_EMAIL", BASELINE_EMAIL),
            ("GIT_AUTHOR_DATE", BASELINE_DATE),
            ("GIT_COMMITTER_NAME", BASELINE_NAME),
            ("GIT_COMMITTER_EMAIL", BASELINE_EMAIL),
            ("GIT_COMMITTER_DATE", BASELINE_DATE),
        ],
    )?;

    verify_task_repository(root, &global_config)
}

fn remove_existing_git_dir(root: &Path) -> Result<(), String> {
    let git_dir = root.join(".git");
    let metadata = match fs::symlink_metadata(&git_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("could not inspect {}: {error}", git_dir.display())),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(&git_dir)
    } else {
        fs::remove_file(&git_dir)
    }
    .map_err(|error| {
        format!(
            "could not reset runner-owned {}: {error}",
            git_dir.display()
        )
    })
}

fn verify_task_repository(root: &Path, global_config: &Path) -> Result<(), String> {
    let top_level = run_checked(
        root,
        &[
            OsString::from("rev-parse"),
            OsString::from("--show-toplevel"),
        ],
        global_config,
        &[],
    )?;
    let reported = PathBuf::from(String::from_utf8_lossy(&top_level.stdout).trim());
    let expected = fs::canonicalize(root)
        .map_err(|error| format!("could not canonicalize task root: {error}"))?;
    let actual = fs::canonicalize(&reported).map_err(|error| {
        format!(
            "could not canonicalize Git top-level {}: {error}",
            reported.display()
        )
    })?;
    if actual != expected {
        return Err(format!(
            "Git top-level escaped the task root (expected {}, got {})",
            expected.display(),
            actual.display()
        ));
    }

    let status = run_checked(
        root,
        &[
            OsString::from("status"),
            OsString::from("--porcelain=v1"),
            OsString::from("--untracked-files=all"),
        ],
        global_config,
        &[],
    )?;
    if !status.stdout.is_empty() {
        return Err(format!(
            "task baseline is not clean:\n{}",
            String::from_utf8_lossy(&status.stdout).trim()
        ));
    }

    let remotes = run_checked(root, &[OsString::from("remote")], global_config, &[])?;
    if !remotes.stdout.is_empty() {
        return Err(format!(
            "task repository unexpectedly has remotes: {}",
            String::from_utf8_lossy(&remotes.stdout).trim()
        ));
    }
    Ok(())
}

fn run_checked(
    cwd: &Path,
    args: &[OsString],
    global_config: &Path,
    env: &[(&str, &str)],
) -> Result<Output, String> {
    let mut command = Command::new("git");
    command
        .args(args.iter().map(OsString::as_os_str))
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", global_config)
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS");
    clear_git_environment(&mut command);
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().map_err(|error| {
        format!(
            "git {} could not start: {error}",
            display_args(args.iter().map(OsString::as_os_str))
        )
    })?;
    if output.status.success() {
        return Ok(output);
    }
    Err(format!(
        "git {} failed: {}",
        display_args(args.iter().map(OsString::as_os_str)),
        git_diagnostic(output.status.code(), &output.stderr)
    ))
}

fn display_args<'a>(args: impl Iterator<Item = &'a OsStr>) -> String {
    args.map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn git_diagnostic(status: Option<i32>, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let detail = stderr.trim();
    match (status, detail.is_empty()) {
        (Some(code), false) => format!("exit code {code}: {detail}"),
        (Some(code), true) => format!("exit code {code} with no stderr"),
        (None, false) => detail.to_string(),
        (None, true) => "could not start git".to_string(),
    }
}
