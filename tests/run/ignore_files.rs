//! Framework-staged files stay out of the project's own lint and format scope,
//! identically in both comparison arms (issue #296).

use crate::codebase_support::{codebase_repo, commit, evals_with_codebase, git};
use crate::helpers::*;
use std::fs;
use std::path::{Path, PathBuf};

/// A codebase that runs Prettier, the shape the shipped Weeknight fixture has:
/// a Prettier config and no `.prettierignore` of its own.
fn prettier_codebase(root: &Path) -> PathBuf {
    let repo = codebase_repo(root, "origin", "main");
    fs::write(repo.join(".prettierrc.json"), "{}\n").unwrap();
    commit(&repo, "add prettier config");
    repo
}

fn run_against(cwd: &Path, skill_dir: &Path, extra: &[&str]) {
    skill_eval()
        .current_dir(cwd)
        .args(["run", "--skill-dir"])
        .arg(skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--harness",
            "claude-code",
            "--no-guard",
            "--dry-run",
        ])
        .args(extra)
        .assert()
        .success();
}

fn prepare(tmp: &Path, codebase_json: &str) -> (PathBuf, PathBuf) {
    let (skill_dir, cwd) = setup(tmp, &evals_with_codebase(codebase_json));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();
    (skill_dir, cwd)
}

#[test]
fn both_arms_get_the_same_ignore_file_hiding_the_staged_skills() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = prettier_codebase(tmp.path());
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = prepare(tmp.path(), &source);

    run_against(&cwd, &skill_dir, &[]);

    let with = cli_env_dir(&cwd, "g1", "with_skill").join(".prettierignore");
    let without = cli_env_dir(&cwd, "g1", "without_skill").join(".prettierignore");
    let with_body = fs::read_to_string(&with).unwrap();
    let without_body = fs::read_to_string(&without).unwrap();

    assert_eq!(
        with_body, without_body,
        "the arms disagree about what the project's formatter sees"
    );
    for entry in [
        "/.eval-magic-outputs/",
        "/tmp/",
        "/.claude/skills/",
        "/.claude/settings.local.json",
    ] {
        assert!(
            with_body.contains(entry),
            "missing {entry} in:\n{with_body}"
        );
    }
}

#[test]
fn the_ignore_file_is_part_of_the_baseline_so_it_never_shows_up_as_agent_work() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = prettier_codebase(tmp.path());
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = prepare(tmp.path(), &source);

    run_against(&cwd, &skill_dir, &[]);

    for condition in ["with_skill", "without_skill"] {
        let env = cli_env_dir(&cwd, "g1", condition);
        assert_eq!(
            git(&env, &["status", "--porcelain", "--untracked-files=all"]),
            "",
            "{condition}: the environment is dirty before dispatch"
        );
        let committed = git(&env, &["show", "refs/eval-magic/baseline:.prettierignore"]);
        assert!(
            committed.contains("/.claude/skills/"),
            "{condition}: the ignore file is not in the baseline: {committed}"
        );
    }
}

#[test]
fn an_empty_ignore_files_declaration_leaves_the_codebase_untouched() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = prettier_codebase(tmp.path());
    let source = format!(
        r#"{{ "url": "{}", "ref": "main", "ignore_files": [] }}"#,
        wire_path(&origin)
    );
    let (skill_dir, cwd) = prepare(tmp.path(), &source);

    run_against(&cwd, &skill_dir, &[]);

    for condition in ["with_skill", "without_skill"] {
        assert!(
            !cli_env_dir(&cwd, "g1", condition)
                .join(".prettierignore")
                .exists(),
            "{condition}: wrote an ignore file the eval opted out of"
        );
    }
}

#[test]
fn a_declared_ignore_file_is_written_where_the_eval_asked() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = prettier_codebase(tmp.path());
    let source = format!(
        r#"{{ "url": "{}", "ref": "main", "ignore_files": ["tooling/.prettierignore"] }}"#,
        wire_path(&origin)
    );
    let (skill_dir, cwd) = prepare(tmp.path(), &source);

    run_against(&cwd, &skill_dir, &[]);

    for condition in ["with_skill", "without_skill"] {
        let env = cli_env_dir(&cwd, "g1", condition);
        assert!(
            !env.join(".prettierignore").exists(),
            "{condition}: detection ran even though the eval declared its own list"
        );
        let body = fs::read_to_string(env.join("tooling/.prettierignore")).unwrap();
        assert!(body.contains("/.claude/skills/"), "{condition}: {body}");
    }
}

#[test]
fn a_codebase_with_no_matching_tooling_gets_no_ignore_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = prepare(tmp.path(), &source);

    run_against(&cwd, &skill_dir, &[]);

    let env = cli_env_dir(&cwd, "g1", "with_skill");
    for name in [".prettierignore", ".eslintignore", ".dockerignore"] {
        assert!(!env.join(name).exists(), "invented {name}");
    }
}
