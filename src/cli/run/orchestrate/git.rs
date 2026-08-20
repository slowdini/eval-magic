//! Runner-owned Git lifecycle for private task environments.

use std::fs;
use std::path::{Path, PathBuf};

use crate::adapters::registry::all_config_dir_names;
use crate::core::{BASELINE_REF, GitOutput, IsolatedGit, run_git};
use crate::source::INITIALIZED_BRANCH;

use super::super::RunError;
use super::super::fixtures::fixture_pairs;
use super::Resolved;
use super::envs::{EnvLayoutInput, env_targets};
use crate::core::RunContext;

const BASELINE_MESSAGE: &str = "eval-magic task baseline";
const BASELINE_NAME: &str = "eval-magic";
const BASELINE_EMAIL: &str = "eval-magic@localhost";
const BASELINE_DATE: &str = "2000-01-01T00:00:00Z";

/// Windows' `MAX_PATH` (260) counts the terminating NUL, so 259 characters are
/// what a tool that is not long-path aware can actually use.
const WINDOWS_USABLE_PATH: usize = 259;

/// Length of what a run writes below a task root before its deepest file,
/// `\.claude\skills\<staged slug>\SKILL.md`: 68 characters for a short slug, 85
/// for a long skill and condition pair, rounded up.
const STAGED_SUFFIX_BUDGET: usize = 96;

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
        let codebase = resolved.codebase_for(&target.eval_ids)?;
        let plan = TaskRepository {
            root: target.root.clone(),
            // A sourced environment already *is* a repository, carrying the
            // history the clone brought with it.
            sourced: codebase.is_some(),
            branch: codebase.map_or_else(
                || INITIALIZED_BRANCH.to_string(),
                |codebase| codebase.source.branch.clone(),
            ),
            forced_paths: runner_placed_paths(ctx, resolved, &target)?,
        };
        initialize_task_repository(&plan).map_err(|error| {
            let hint = path_budget_hint(&target.root, cfg!(windows))
                .map(|hint| format!("\n{hint}"))
                .unwrap_or_default();
            RunError::msg(format!(
                "could not initialize task Git repository at {}: {error}{hint}",
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
/// the declared fixture overlay — are forced on top of it.
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
        for (dest, _source) in fixture_pairs(eval, &ctx.skill_subdir)? {
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
    /// Whether a codebase already put a repository here. A sourced environment
    /// keeps its `.git`; a fixture-only one is initialized from nothing.
    sourced: bool,
    branch: String,
    forced_paths: Vec<String>,
}

/// A sentence naming the Windows path budget, for a task root too deep to hold
/// what a run stages below it.
///
/// Measures the root rather than matching git's `Filename too long`, which is a
/// localizable `strerror` mapping.
fn path_budget_hint(root: &Path, windows: bool) -> Option<String> {
    let length = root.as_os_str().to_string_lossy().chars().count();
    if !windows || length + STAGED_SUFFIX_BUDGET <= WINDOWS_USABLE_PATH {
        return None;
    }
    Some(format!(
        "This task root is {length} characters and a run stages roughly \
         {STAGED_SUFFIX_BUDGET} more below it, past the {WINDOWS_USABLE_PATH} Windows \
         allows a tool that is not long-path aware. If the failure above names a path \
         or filename length, re-run from a shorter workspace root."
    ))
}

fn initialize_task_repository(plan: &TaskRepository) -> Result<(), String> {
    let root = plan.root.as_path();
    let git = IsolatedGit::new()?;

    if plan.sourced {
        // The clone's history is the point of sourcing a codebase, so this is
        // the one case that must not reset `.git`.
        strip_remotes(root, &git)?;
    } else {
        remove_existing_git_dir(root)?;
        let template = git.template_dir().to_string_lossy().into_owned();
        run_checked(
            &git,
            root,
            &[
                "init",
                "--quiet",
                "--initial-branch",
                &plan.branch,
                "--template",
                &template,
                ".",
            ],
            &[],
        )?;
    }

    let hooks_dir = root.join(".git/eval-magic-disabled-hooks");
    fs::create_dir_all(root.join(".git/info"))
        .map_err(|error| format!("could not create Git info directory: {error}"))?;
    fs::create_dir_all(&hooks_dir)
        .map_err(|error| format!("could not create empty Git hooks directory: {error}"))?;
    fs::write(root.join(".git/info/exclude"), "/.eval-magic-outputs/\n")
        .map_err(|error| format!("could not configure framework output exclusion: {error}"))?;

    let hooks_path = hooks_dir.to_string_lossy().into_owned();
    for (name, value) in [
        ("user.name", BASELINE_NAME),
        ("user.email", BASELINE_EMAIL),
        ("commit.gpgSign", "false"),
        ("tag.gpgSign", "false"),
        ("core.hooksPath", hooks_path.as_str()),
        // Lifts Windows' `MAX_PATH`, which a staged skill under a deep workspace
        // crosses. Task repositories run under isolated Git configuration, so an
        // operator's own setting never reaches one. Written to the repository,
        // not per invocation, so the agent under test and the pipeline inherit
        // it; git ignores the key off Windows.
        ("core.longpaths", "true"),
    ] {
        run_checked(&git, root, &["config", "--local", name, value], &[])?;
    }

    // Respects the sourced codebase's `.gitignore`: a real repository ignores
    // its build output, and a forced add here would commit `target/` or
    // `node_modules/` into the baseline every environment starts from.
    //
    // No exclude pathspec for `.eval-magic-outputs`: `.git/info/exclude` above
    // already ignores it, and an unforced add honors that. The pathspecs this
    // replaces existed only to carve it back out of a forced add.
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::core::runtime::report_skip;

    /// A repository with no codebase behind it — the shape these path-budget
    /// tests exercise, and what a fixture-only run has always produced.
    fn fixture_only(root: &Path) -> TaskRepository {
        TaskRepository {
            root: root.to_path_buf(),
            sourced: false,
            branch: INITIALIZED_BRANCH.to_string(),
            forced_paths: Vec::new(),
        }
    }

    /// A staged skill's path relative to its task root: 68 characters, the
    /// shortest realistic shape of `.claude/skills/<staged slug>/SKILL.md`.
    const STAGED_SKILL: &str =
        ".claude/skills/slow-powers-eval-1-with_skill__widget-skill/SKILL.md";

    /// `base` extended with padding components until it is `target` characters
    /// long (or left as it is, when it is already longer).
    fn padded_to(base: &Path, target: usize) -> PathBuf {
        const PAD: &str = "eval-magic-path-budget-padding";
        let mut root = base.to_path_buf();
        while root.as_os_str().len() + 1 + PAD.len() <= target {
            root = root.join(PAD);
        }
        let remaining = target.saturating_sub(root.as_os_str().len() + 1);
        if remaining > 0 {
            root = root.join(&PAD[..remaining]);
        }
        root
    }

    /// `base` spelled the way git will report it.
    ///
    /// `std::env::temp_dir()` can hand back an 8.3 short name — `RUNNER~1` for
    /// `runneradmin` on a GitHub runner — which git expands before it measures.
    /// Those three characters are invisible to a length computed from the short
    /// spelling, and three is enough to push `.git/config` past the limit on a
    /// host where the same target fits locally. Measure what git measures.
    fn long_form(base: &Path) -> PathBuf {
        let Ok(canonical) = base.canonicalize() else {
            return base.to_path_buf();
        };
        let text = canonical.to_string_lossy().into_owned();
        // Canonicalising on Windows yields a `\\?\` verbatim path; git reports
        // the plain spelling, so drop the prefix to keep the two comparable.
        PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
    }

    /// A `target`-character task root holding a staged `SKILL.md`, or `None`
    /// when this host cannot write that deep. The probe is the same `std::fs`
    /// write staging performs, so the gate is the capability, not the OS.
    fn deep_task_root(base: &Path, target: usize, test: &str) -> Option<PathBuf> {
        let root = padded_to(&long_form(base), target);
        let staged = root.join(STAGED_SKILL);
        let written = fs::create_dir_all(staged.parent().expect("the staged path has a parent"))
            .and_then(|()| fs::write(&staged, "---\nname: widget-skill\n---\n\nbody\n"));
        if let Err(error) = written {
            report_skip(
                test,
                &format!(
                    "this host cannot create a {}-character path ({error})",
                    staged.as_os_str().len()
                ),
            );
            return None;
        }
        Some(root)
    }

    /// Rust's filesystem calls pass verbatim paths, so a deep workspace stages
    /// its skill fine and only git meets Windows' `MAX_PATH` — the baseline
    /// `git add` aborts with `Filename too long`.
    #[test]
    fn task_repository_initializes_when_the_staged_skill_exceeds_the_windows_path_limit() {
        let test =
            "task_repository_initializes_when_the_staged_skill_exceeds_the_windows_path_limit";
        let tmp = tempfile::TempDir::new().unwrap();
        // 195 characters puts the staged path past the budget while the
        // repository's own `.git` bookkeeping stays under it.
        let Some(root) = deep_task_root(tmp.path(), 195, test) else {
            return;
        };
        assert!(
            root.join(STAGED_SKILL).as_os_str().len() > WINDOWS_USABLE_PATH,
            "the fixture must exceed the Windows path budget to exercise anything"
        );
        initialize_task_repository(&fixture_only(&root))
            .expect("a task root with a deep staged skill initializes");
    }

    /// A failure under a deep root has to name the path budget: git reports
    /// `Filename too long` about one file, which says nothing about the
    /// workspace root being the thing to shorten.
    #[test]
    fn path_budget_hint_names_the_budget_for_a_deep_windows_root() {
        let root = padded_to(Path::new("C:/w"), 210);
        let hint = path_budget_hint(&root, true).expect("a deep Windows root gets a hint");
        assert!(hint.contains("210"), "{hint}");
        assert!(hint.contains(&WINDOWS_USABLE_PATH.to_string()), "{hint}");
        assert!(hint.contains("shorter workspace root"), "{hint}");
    }

    /// The hint is a Windows path-budget explanation, so it stays out of the way
    /// of every failure it cannot explain.
    #[test]
    fn path_budget_hint_stays_silent_off_windows_and_for_short_roots() {
        let deep = padded_to(Path::new("C:/w"), 210);
        assert_eq!(path_budget_hint(&deep, false), None);
        assert_eq!(path_budget_hint(Path::new("C:/w/iteration-1"), true), None);
    }

    /// Past a certain depth the failure goes quiet: git cannot open the staged
    /// directory to enumerate it, so `git add` warns, exits zero, and leaves the
    /// skill under test out of the baseline that later diffs are measured
    /// against. Nothing downstream can flag a file git could not read.
    #[test]
    fn task_repository_baseline_tracks_a_staged_skill_past_the_windows_path_limit() {
        let test = "task_repository_baseline_tracks_a_staged_skill_past_the_windows_path_limit";
        let tmp = tempfile::TempDir::new().unwrap();
        // 202 characters isolates the quiet mode: enumerating the staged
        // directory needs 261, past the budget, while the repository's own loose
        // objects still fit at 256.
        let Some(root) = deep_task_root(tmp.path(), 202, test) else {
            return;
        };
        initialize_task_repository(&fixture_only(&root))
            .expect("a task root in the quiet band initializes");
        let tracked = run_git(&["ls-files"], &root);
        assert!(
            String::from_utf8_lossy(&tracked.stdout).contains("SKILL.md"),
            "the baseline commit must track the staged skill, not skip it"
        );
    }

    /// A root deep enough that `.git/objects/pack` crosses the budget: `git
    /// init` creates it before any repository-local configuration exists, so the
    /// lift has to reach that invocation too.
    ///
    /// 244 is not arbitrary and not the maximum. Git's long-path awareness is
    /// per-operation: creating `.git/objects/pack` survives well past the
    /// budget, `git init` writing `.git/config` stops at exactly
    /// `WINDOWS_USABLE_PATH`, and the `git config --local` that follows gives up
    /// two characters earlier still. 244 puts `pack` at 262 — past the budget,
    /// which is the point — while leaving `.git/config` at 256, a deliberate
    /// three inside the tightest of those ceilings. The assertions below pin the
    /// window so a future edit cannot silently slide the root out of it.
    #[test]
    fn task_repository_initializes_when_its_git_directory_exceeds_the_windows_path_limit() {
        const GIT_CONFIG: &str = ".git/config";
        const GIT_PACK: &str = ".git/objects/pack";
        let test =
            "task_repository_initializes_when_its_git_directory_exceeds_the_windows_path_limit";
        let tmp = tempfile::TempDir::new().unwrap();
        let Some(root) = deep_task_root(tmp.path(), 244, test) else {
            return;
        };

        let length = root.as_os_str().len();
        assert!(
            length + 1 + GIT_PACK.len() > WINDOWS_USABLE_PATH,
            "{length}-character root leaves `.git/objects/pack` inside the budget, testing nothing"
        );
        assert!(
            length + 1 + GIT_CONFIG.len() + 3 <= WINDOWS_USABLE_PATH,
            "{length}-character root leaves `.git/config` no margin below the budget"
        );
        initialize_task_repository(&fixture_only(&root))
            .expect("a task root deeper than `.git` needs initializes");
    }
}
