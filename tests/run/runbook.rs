//! `RUNBOOK.md` generation during `run`: the followable isolated-session handoff
//! artifact, and the post-run pointer at it.

use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

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

    // Issue #248: the runbook is the manual for a campaign, so it names the
    // shell it expects — and names it *above* the first command a reader would
    // paste, which is the whole point of stating the requirement at all.
    let requirement = book
        .find("Git Bash")
        .expect("the runbook states the POSIX shell requirement");
    assert!(book.contains("jq"), "the requirement names jq too: {book}");
    assert!(book.contains("WSL"), "{book}");
    assert!(
        requirement < book.find("claude -p").unwrap(),
        "the requirement precedes the first pasteable command: {book}"
    );
}

/// Issue #248: `run` used to succeed on a host with no POSIX shell and print
/// recipes only a POSIX shell can execute, with nothing to say a different shell
/// was expected. The prepared workspace is still correct, so this warns rather
/// than failing — but it must warn, and it must name the way out.
///
/// `EVAL_MAGIC_SH` pointing at nothing reproduces the shell-less host on every
/// platform, so the test does not depend on what the developer has installed.
#[test]
fn run_warns_when_the_host_has_no_posix_shell() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .env("EVAL_MAGIC_SH", tmp.path().join("no-shell-here"))
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
        .success()
        .stderr(contains("⚠").and(contains("Git Bash")).and(contains("WSL")));
}

#[test]
fn run_writes_headless_runbook_for_opencode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "opencode", "--dry-run"])
        .assert()
        .success()
        .stderr(contains("declares no dispatch exec recipe").not());

    let book = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    assert!(
        book.contains("opencode run --dir"),
        "carries the opencode CLI dispatch recipe: {book}"
    );
    assert!(
        book.contains("--harness opencode"),
        "pipeline commands carry --harness opencode: {book}"
    );
    assert!(!book.contains("{{"), "no unsubstituted tokens: {book}");

    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(
        manifest.contains("opencode run --dir"),
        "the manifest carries the same recipe: {manifest}"
    );
    assert!(
        !manifest.contains("{{"),
        "no unsubstituted tokens: {manifest}"
    );
    // The manifest carries the same POSIX recipes, so it carries the same
    // requirement (issue #248 names both artifacts).
    assert!(
        manifest.contains("Git Bash") && manifest.contains("jq"),
        "the manifest states the POSIX tooling requirement: {manifest}"
    );
}
