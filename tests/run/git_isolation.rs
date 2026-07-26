use crate::helpers::*;
use std::fs;
use std::path::Path;
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

fn git_commit_all(cwd: &Path, message: &str) {
    git(cwd, &["add", "--all"]);
    git(
        cwd,
        &[
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            message,
        ],
    );
}

#[test]
fn every_task_is_a_clean_local_git_repo_inside_a_dirty_ignored_parent_repo() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    git(&cwd, &["init", "-b", "parent"]);
    fs::write(cwd.join(".gitignore"), ".eval-magic/\n").unwrap();
    fs::write(cwd.join("tracked.txt"), "before\n").unwrap();
    git_commit_all(&cwd, "parent baseline");
    fs::write(cwd.join("tracked.txt"), "dirty parent\n").unwrap();
    let parent_status = git(&cwd, &["status", "--porcelain"]);

    skill_eval()
        .current_dir(&cwd)
        .env("GIT_AUTHOR_NAME", "Inherited Author")
        .env("GIT_AUTHOR_EMAIL", "inherited-author@example.com")
        .env("GIT_COMMITTER_NAME", "Inherited Committer")
        .env("GIT_COMMITTER_EMAIL", "inherited-committer@example.com")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "user.name")
        .env("GIT_CONFIG_VALUE_0", "Injected Config Name")
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let first_root = Path::new(
        dispatch["tasks"].as_array().unwrap()[0]["eval_root"]
            .as_str()
            .unwrap(),
    )
    .to_path_buf();
    for task in dispatch["tasks"].as_array().unwrap() {
        let eval_root = Path::new(task["eval_root"].as_str().unwrap());
        assert_eq!(
            fs::canonicalize(git(eval_root, &["rev-parse", "--show-toplevel"])).unwrap(),
            fs::canonicalize(eval_root).unwrap()
        );
        assert_eq!(git(eval_root, &["symbolic-ref", "--short", "HEAD"]), "work");
        assert_eq!(git(eval_root, &["status", "--porcelain"]), "");
        assert_eq!(git(eval_root, &["remote"]), "");
        assert_eq!(
            git(
                eval_root,
                &["check-ignore", ".eval-magic-outputs/probe.txt"]
            ),
            ".eval-magic-outputs/probe.txt"
        );
        assert_eq!(
            git(
                eval_root,
                &["log", "-1", "--format=%an|%ae|%aI|%cn|%ce|%cI|%s"]
            ),
            "eval-magic|eval-magic@localhost|2000-01-01T00:00:00Z|\
             eval-magic|eval-magic@localhost|2000-01-01T00:00:00Z|\
             eval-magic task baseline"
        );
    }

    fs::write(first_root.join("agent-edit.txt"), "changed\n").unwrap();
    assert!(
        git(&first_root, &["status", "--porcelain"]).contains("agent-edit.txt"),
        "task edits must be visible to local Git"
    );
    git_commit_all(&first_root, "agent commit");
    assert_eq!(git(&first_root, &["status", "--porcelain"]), "");
    assert_eq!(
        git(&cwd, &["status", "--porcelain"]),
        parent_status,
        "preparing task repositories must not alter the parent repository"
    );
}

#[test]
fn explicit_iteration_rebuild_resets_task_git_history_branch_and_remotes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let run = |iteration: bool| {
        let mut command = skill_eval();
        command
            .current_dir(&cwd)
            .args(["run", "--skill-dir"])
            .arg(&skill_dir)
            .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"]);
        if iteration {
            command.args(["--iteration", "1"]);
        }
        command.assert().success();
    };
    run(false);

    let task_root = cli_env_dir(&cwd, "g1", "with_skill");
    fs::write(task_root.join("extra.txt"), "extra\n").unwrap();
    git_commit_all(&task_root, "extra history");
    git(&task_root, &["switch", "-c", "leaked-branch"]);
    git(
        &task_root,
        &["remote", "add", "origin", "https://example.com/repo.git"],
    );
    assert_eq!(git(&task_root, &["rev-list", "--count", "HEAD"]), "2");

    run(true);

    assert_eq!(
        git(&task_root, &["symbolic-ref", "--short", "HEAD"]),
        "work"
    );
    assert_eq!(git(&task_root, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(
        git(&task_root, &["log", "-1", "--format=%s"]),
        "eval-magic task baseline"
    );
    assert_eq!(git(&task_root, &["remote"]), "");
    assert_eq!(git(&task_root, &["status", "--porcelain"]), "");
}

#[test]
fn nested_git_repository_fixture_is_preserved_inside_the_task_repo() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review", "expected_output": "a review",
          "files": ["vendor"] } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    let vendor = skill_dir.join("mr-review/evals/vendor");
    fs::create_dir_all(&vendor).unwrap();
    git(&vendor, &["init", "-b", "nested"]);
    fs::write(vendor.join("source.txt"), "nested source\n").unwrap();
    git_commit_all(&vendor, "nested baseline");

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let task_root = cli_env_dir(&cwd, "g1", "with_skill");
    assert!(task_root.join("vendor/.git/config").exists());
    assert_eq!(
        read_str(&task_root.join("vendor/source.txt")),
        "nested source\n"
    );
    assert_eq!(git(&task_root, &["status", "--porcelain"]), "");
}
