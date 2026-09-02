//! Runner-owned Git lifecycle for private task environments.

use std::fs;
use std::path::{Path, PathBuf};

use super::super::RunError;
use super::super::overlays::overlay_file_pairs;
use super::Resolved;
use super::envs::{EnvLayoutInput, env_targets};
use crate::adapters::registry::all_config_dir_names;
use crate::core::RunContext;
use crate::core::{BASELINE_REF, GitOutput, IsolatedGit, run_git};

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

pub(super) fn initialize_task_repositories(
    ctx: &RunContext,
    resolved: &Resolved,
) -> Result<(), RunError> {
    let targets = env_targets(&EnvLayoutInput {
        iteration_dir: &resolved.iteration_dir,
        groups: &resolved.groups,
        cond_a: resolved.cond_a,
        cond_b: resolved.cond_b,
        skill_path_a: resolved.skill_path_a.as_deref(),
        skill_path_b: resolved.skill_path_b.as_deref(),
    });
    for target in targets {
        resolved.codebase_for(&target.eval_ids)?;
        let plan = TaskRepository {
            root: target.root.clone(),
            forced_paths: runner_placed_paths(ctx, resolved, &target)?,
        };
        initialize_task_repository(&plan).map_err(|error| {
            RunError::msg(format!(
                "could not initialize task Git repository at {}: {error}",
                target.root.display()
            ))
        })?;
    }
    Ok(())
}

/// Env-relative paths the runner placed, which must reach the baseline commit
/// even when the sourced codebase's own `.gitignore` covers them.
///
/// A real repository ignores its build output, and a blanket forced add would
/// sweep `target/` or `node_modules/` into the baseline. So the baseline add
/// respects `.gitignore` and these paths — the harness config directories, and
/// the declared file overlay — are forced on top of it.
fn runner_placed_paths(
    ctx: &RunContext,
    resolved: &Resolved,
    target: &super::envs::EnvTarget,
) -> Result<Vec<String>, RunError> {
    let mut paths: Vec<String> = all_config_dir_names()
        .into_iter()
        .filter(|name| target.root.join(name).exists())
        .collect();
    for eval_id in &target.eval_ids {
        let Some(eval) = resolved
            .selected_evals
            .iter()
            .find(|candidate| &candidate.id == eval_id)
        else {
            continue;
        };
        for (dest, _source) in overlay_file_pairs(eval, &ctx.skill_subdir)? {
            if target.root.join(&dest).exists() {
                paths.push(dest);
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// One task repository to establish.
struct TaskRepository {
    root: PathBuf,
    forced_paths: Vec<String>,
}

fn initialize_task_repository(plan: &TaskRepository) -> Result<(), String> {
    let root = plan.root.as_path();
    let git = IsolatedGit::new()?;

    // Materialization wraps even a historyless directory in a repository.
    // Preserve that source history while severing every route back to it.
    strip_remotes(root, &git)?;

    let hooks_dir = root.join(".git/eval-magic-disabled-hooks");
    fs::create_dir_all(root.join(".git/info"))
        .map_err(|error| format!("could not create Git info directory: {error}"))?;
    fs::create_dir_all(&hooks_dir)
        .map_err(|error| format!("could not create empty Git hooks directory: {error}"))?;
    let mut exclude = crate::sandbox::framework_owned_entries().join("\n");
    exclude.push('\n');
    fs::write(root.join(".git/info/exclude"), exclude)
        .map_err(|error| format!("could not configure framework path exclusion: {error}"))?;

    let hooks_path = hooks_dir.to_string_lossy().into_owned();
    for (name, value) in [
        ("user.name", BASELINE_NAME),
        ("user.email", BASELINE_EMAIL),
        ("commit.gpgSign", "false"),
        ("tag.gpgSign", "false"),
        ("core.hooksPath", hooks_path.as_str()),
    ] {
        run_checked(&git, root, &["config", "--local", name, value], &[])?;
    }

    // Respects the sourced codebase's `.gitignore`: a real repository ignores
    // its build output, and a forced add here would commit `target/` or
    // `node_modules/` into the baseline every environment starts from.
    //
    // No exclude pathspec for the framework-owned paths: `.git/info/exclude`
    // above already ignores them, and an unforced add honors that. The pathspecs
    // this replaces existed only to carve them back out of a forced add.
    run_checked(&git, root, &["add", "--all", "--", "."], &[])?;
    // What the runner itself placed is forced in on top, so a codebase that
    // ignores `.claude/` cannot hide the staged skill from the baseline — which
    // would leave the condition under test outside every later diff.
    if !plan.forced_paths.is_empty() {
        let mut args = vec!["add", "--force", "--"];
        args.extend(plan.forced_paths.iter().map(String::as_str));
        run_checked(&git, root, &args, &[])?;
    }
    run_checked(
        &git,
        root,
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            BASELINE_MESSAGE,
        ],
        &[
            ("GIT_AUTHOR_NAME", BASELINE_NAME),
            ("GIT_AUTHOR_EMAIL", BASELINE_EMAIL),
            ("GIT_AUTHOR_DATE", BASELINE_DATE),
            ("GIT_COMMITTER_NAME", BASELINE_NAME),
            ("GIT_COMMITTER_EMAIL", BASELINE_EMAIL),
            ("GIT_COMMITTER_DATE", BASELINE_DATE),
        ],
    )?;

    // The start state, named. Everything the agent does afterwards is measurable
    // as the difference from this ref, whether the environment has one commit or
    // a codebase's entire history behind it.
    //
    // Deliberately outside `refs/heads/`: it never appears in `git branch`, so
    // it adds nothing to what the agent under test sees.
    run_checked(&git, root, &["update-ref", BASELINE_REF, "HEAD"], &[])?;

    verify_task_repository(root, &git)
}

/// Drop every remote, so nothing in the environment can reach the source it was
/// cloned from — or push to it.
fn strip_remotes(root: &Path, git: &IsolatedGit) -> Result<(), String> {
    let listed = run_checked(git, root, &["remote"], &[])?;
    let remotes: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    for remote in &remotes {
        run_checked(git, root, &["remote", "remove", remote], &[])?;
    }
    Ok(())
}

fn verify_task_repository(root: &Path, git: &IsolatedGit) -> Result<(), String> {
    let top_level = run_checked(git, root, &["rev-parse", "--show-toplevel"], &[])?;
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
        git,
        root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        &[],
    )?;
    if !status.stdout.is_empty() {
        return Err(format!(
            "task baseline is not clean:\n{}",
            String::from_utf8_lossy(&status.stdout).trim()
        ));
    }

    let remotes = run_checked(git, root, &["remote"], &[])?;
    if !remotes.stdout.is_empty() {
        return Err(format!(
            "task repository unexpectedly has remotes: {}",
            String::from_utf8_lossy(&remotes.stdout).trim()
        ));
    }
    Ok(())
}

fn run_checked(
    git: &IsolatedGit,
    cwd: &Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<GitOutput, String> {
    let output = git.run(cwd, args, env);
    match output.status {
        Some(0) => Ok(output),
        status => Err(format!(
            "git {} failed: {}",
            args.join(" "),
            git_diagnostic(status, &output.stderr)
        )),
    }
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
