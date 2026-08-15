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

/// Windows' `MAX_PATH` (260) counts the terminating NUL, so 259 characters are
/// what a tool that is not long-path aware can actually use.
const WINDOWS_USABLE_PATH: usize = 259;

/// What a run still writes below a task root before its deepest file:
/// `\.claude\skills\<staged slug>\SKILL.md` — 68 characters for the short slug
/// in issue #270, 85 for a long skill and condition pair. Rounded up so the
/// hint below arrives while the budget is nearly gone rather than after.
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

/// A sentence naming the Windows path budget, for a task root already deep
/// enough that what a run stages below it will not fit.
///
/// Keyed on the measured root rather than on git's `Filename too long`: that
/// wording is git's own `strerror` mapping, so matching it would tie the hint to
/// one locale. The root's length is a local fact, which lets the hint stay
/// definite about the budget and hedged about the cause.
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
        // A staged skill under a deep workspace crosses Windows' `MAX_PATH`
        // (issue #270), and the configuration isolation above discards the
        // `core.longpaths` an operator set globally — so the runner writes its
        // own. It belongs in the repository rather than on each invocation: the
        // agent under test runs its own git in here, and the pipeline reads the
        // repository back through `run_git`, so both inherit it. Written on
        // every host; git ignores the key off Windows.
        ("core.longpaths", OsString::from("true")),
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
        // `git init` creates `.git/objects/pack` before the repository-local
        // `core.longpaths` exists to lift it, so that one invocation needs the
        // setting passed transiently.
        .args(["-c", "core.longpaths=true"])
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::core::runtime::report_skip;

    /// The staged path issue #270 failed on, relative to a task root: 68
    /// characters, the shortest realistic shape of
    /// `.claude/skills/<staged slug>/SKILL.md`.
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

    /// A `target`-character task root holding a staged `SKILL.md`, or `None`
    /// when this host cannot write that deep.
    ///
    /// Gated on the capability rather than the OS: the probe is the same
    /// `std::fs` write staging performs, so a host that cannot do it says so
    /// instead of failing somewhere inside git.
    fn deep_task_root(base: &Path, target: usize, test: &str) -> Option<PathBuf> {
        let root = padded_to(base, target);
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

    /// Issue #270: the run staged its skill correctly — Rust's own filesystem
    /// calls pass verbatim paths, which lifts Windows' `MAX_PATH` — and then
    /// aborted at the baseline `git add` with `Filename too long`. The
    /// runner-owned repository has to carry `core.longpaths` itself: its
    /// deliberate configuration isolation discards the one an operator set
    /// globally.
    #[test]
    fn task_repository_initializes_when_the_staged_skill_exceeds_the_windows_path_limit() {
        let test =
            "task_repository_initializes_when_the_staged_skill_exceeds_the_windows_path_limit";
        let tmp = tempfile::TempDir::new().unwrap();
        // 195 characters puts the staged path past the 259 Windows allows a
        // caller that is not long-path aware, while the repository's own `.git`
        // bookkeeping stays under it — the shape reported in #270.
        let Some(root) = deep_task_root(tmp.path(), 195, test) else {
            return;
        };
        assert!(
            root.join(STAGED_SKILL).as_os_str().len() > WINDOWS_USABLE_PATH,
            "the fixture must exceed the Windows path budget to exercise anything"
        );
        initialize_task_repository(&root)
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

    /// A few characters deeper the failure goes quiet instead of loud: git can
    /// no longer open the staged directory to enumerate it, so `git add` warns,
    /// exits zero, and leaves the skill under test out of the baseline — and the
    /// cleanliness check that follows cannot report a file git could not read.
    /// The baseline every later diff is measured against would silently lack the
    /// thing under test.
    #[test]
    fn task_repository_baseline_tracks_a_staged_skill_past_the_windows_path_limit() {
        let test = "task_repository_baseline_tracks_a_staged_skill_past_the_windows_path_limit";
        let tmp = tempfile::TempDir::new().unwrap();
        // 202 characters is the narrow band that isolates the quiet mode:
        // enumerating the staged directory needs 261, past the budget, while the
        // repository's own loose objects still fit at 256. Post-fix the
        // assertion holds at any depth; the band is what makes it fail without.
        let Some(root) = deep_task_root(tmp.path(), 202, test) else {
            return;
        };
        initialize_task_repository(&root).expect("a task root in the quiet band initializes");
        let tracked = run_git(&["ls-files"], &root);
        assert!(
            String::from_utf8_lossy(&tracked.stdout).contains("SKILL.md"),
            "the baseline commit must track the staged skill, not skip it"
        );
    }

    /// One step deeper: the repository's own `.git/objects/pack` crosses the
    /// budget too, so `git init` — which runs before the repository-local
    /// configuration exists — has to be long-path aware in its own right.
    #[test]
    fn task_repository_initializes_when_its_git_directory_exceeds_the_windows_path_limit() {
        let test =
            "task_repository_initializes_when_its_git_directory_exceeds_the_windows_path_limit";
        let tmp = tempfile::TempDir::new().unwrap();
        let Some(root) = deep_task_root(tmp.path(), 245, test) else {
            return;
        };
        initialize_task_repository(&root)
            .expect("a task root deeper than `.git` needs initializes");
    }
}
