//! `RUNBOOK.md` generation during `run`: the followable isolated-session handoff
//! artifact, and the post-run pointer at it.

use crate::helpers::*;

#[test]
fn run_writes_headless_runbook_for_codex() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "codex", "--dry-run"])
        .assert()
        .success();

    // Cli dispatches from per-(group, condition) envs, so the human-followed runbook
    // lives in the iteration dir, not inside any env.
    assert!(
        !cli_env_dir(&cwd, "g1", "with_skill")
            .join("RUNBOOK.md")
            .exists()
    );
    let book = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    assert!(
        book.contains("human driving"),
        "frames the run for a human at a terminal: {book}"
    );
    assert!(
        book.contains("codex --ask-for-approval never exec"),
        "carries the Codex CLI dispatch recipe: {book}"
    );
    assert!(
        book.contains("--harness codex"),
        "pipeline commands carry --harness codex: {book}"
    );
    assert!(!book.contains("{{"), "no unsubstituted tokens: {book}");
}

#[test]
fn run_writes_headless_runbook_for_claude() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "claude-code",
            "--dry-run",
        ])
        .assert()
        .success();

    let book = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    // A Claude Code run uses the shared human-followed template carrying the
    // `claude -p` recipe. Each task dispatches from its own per-(group, condition)
    // env, so the runbook lives in the iteration dir, above those envs.
    assert!(
        book.contains("human driving"),
        "frames the run for a human at a terminal: {book}"
    );
    assert!(
        book.contains("claude -p"),
        "carries the claude -p dispatch recipe: {book}"
    );
    assert!(
        !book.contains("switch-condition"),
        "headless does not use the in-session batch loop: {book}"
    );
    assert!(!book.contains("{{"), "no unsubstituted tokens: {book}");
}
