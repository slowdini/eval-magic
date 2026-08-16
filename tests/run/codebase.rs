//! Sourcing a real codebase into each task environment (issue #252).
//!
//! The environments a run builds are asserted here rather than in unit tests
//! because the property under test spans resolution, provisioning, staging, and
//! the fixture overlay — it is only true of a whole prepared workspace.

use crate::helpers::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed in {}:\n{}",
        args.join(" "),
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// A repository usable as a codebase source: two commits on `branch`, with a
/// `.gitignore` that ignores `build/`, and an ignored file already present.
fn codebase_repo(root: &Path, name: &str, branch: &str) -> PathBuf {
    let repo = root.join(name);
    fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch", branch, "."]);
    fs::write(repo.join(".gitignore"), "build/\n").unwrap();
    fs::write(repo.join("src/lib.rs"), "pub fn one() -> u32 { 1 }\n").unwrap();
    commit(&repo, "first");
    fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    commit(&repo, "second");
    fs::create_dir_all(repo.join("build")).unwrap();
    fs::write(repo.join("build/artifact.bin"), "not source\n").unwrap();
    repo
}

fn commit(cwd: &Path, message: &str) {
    git(cwd, &["add", "--all"]);
    git(
        cwd,
        &[
            "-c",
            "user.name=Codebase Author",
            "-c",
            "user.email=codebase@example.com",
            "commit",
            "--quiet",
            "-m",
            message,
        ],
    );
}

/// An evals config whose single eval overlays `TASK.md` onto `codebase`.
fn evals_with_codebase(codebase: &str) -> String {
    format!(
        r#"{{
          "skill_name": "mr-review",
          "codebase": {codebase},
          "evals": [
            {{
              "id": "e1",
              "prompt": "add a function",
              "expected_output": "a function",
              "files": ["TASK.md"]
            }}
          ]
        }}"#
    )
}

#[test]
fn a_git_codebase_arrives_in_every_env_with_history_and_no_remote() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(
        skill_dir.join("mr-review/evals/TASK.md"),
        "Add a `two()` function.\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    for condition in ["with_skill", "without_skill"] {
        let env = cli_env_dir(&cwd, "g1", condition);
        assert_eq!(
            fs::read_to_string(env.join("src/main.rs")).unwrap(),
            "fn main() {}\n",
            "{condition}: the codebase's files must be present"
        );
        assert!(
            git(&env, &["rev-list", "--count", "HEAD"])
                .parse::<u32>()
                .unwrap()
                >= 2,
            "{condition}: the codebase's history must survive provisioning"
        );
        assert_eq!(
            git(&env, &["remote"]),
            "",
            "{condition}: no env may retain a remote"
        );
        // The overlay: a declared fixture lands on top, at its declared path.
        assert_eq!(
            fs::read_to_string(env.join("TASK.md")).unwrap(),
            "Add a `two()` function.\n",
            "{condition}: files are an overlay on the codebase"
        );
    }
}

#[test]
fn the_baseline_ref_marks_the_state_every_codebase_env_starts_from() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let env = cli_env_dir(&cwd, "g1", "with_skill");
    assert_eq!(
        git(&env, &["rev-parse", "refs/eval-magic/baseline"]),
        git(&env, &["rev-parse", "HEAD"]),
        "the baseline ref must name the start state"
    );
    // Outside refs/heads, so it never shows up in what the agent sees.
    assert_eq!(git(&env, &["branch", "--list"]), "* main");
    assert_eq!(
        git(&env, &["status", "--porcelain"]),
        "",
        "the baseline commit must leave nothing uncommitted"
    );
}

/// A real repository ignores its build output. Committing that into the
/// baseline would put megabytes of artifacts in every environment's start
/// state — but the runner's own files have to land regardless of what the
/// codebase ignores, or the condition under test falls outside every diff.
#[test]
fn the_baseline_respects_codebase_gitignore_but_still_tracks_runner_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    // The codebase ignores the harness config dir the runner stages into.
    fs::write(origin.join(".gitignore"), "build/\n.claude/\n").unwrap();
    commit(&origin, "ignore the harness config dir too");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let env = cli_env_dir(&cwd, "g1", "with_skill");
    let tracked = git(&env, &["ls-files"]);

    assert!(
        !tracked.lines().any(|path| path.starts_with("build/")),
        "gitignored build output must stay out of the baseline:\n{tracked}"
    );
    assert!(
        tracked.lines().any(|path| path.starts_with(".claude/")),
        "the staged skill must be tracked even though the codebase ignores .claude/:\n{tracked}"
    );
    assert!(
        tracked.lines().any(|path| path == "TASK.md"),
        "the fixture overlay must be tracked:\n{tracked}"
    );
    assert!(
        !tracked
            .lines()
            .any(|path| path.starts_with(".eval-magic-outputs")),
        "framework output stays excluded:\n{tracked}"
    );
}

/// The ticket's last acceptance criterion: an eval declaring no codebase keeps
/// the environment it has always had.
#[test]
fn a_fixture_only_eval_still_gets_the_repository_it_always_had() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let env = cli_env_dir(&cwd, "g1", "with_skill");
    assert_eq!(git(&env, &["symbolic-ref", "--short", "HEAD"]), "work");
    assert_eq!(git(&env, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(git(&env, &["remote"]), "");
    assert_eq!(git(&env, &["status", "--porcelain"]), "");
    assert_eq!(
        git(&env, &["rev-parse", "refs/eval-magic/baseline"]),
        git(&env, &["rev-parse", "HEAD"])
    );
}
