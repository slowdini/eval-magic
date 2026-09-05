//! `RUNBOOK.md` generation during `run`: the followable isolated-session handoff
//! artifact, and the post-run pointer at it.

use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

/// One dispatch command, whatever the plan holds — a mixed plan of scripted and
/// one-shot evals included. The runner drives every task, so the runbook has no
/// per-plan-shape branch to render.
#[test]
fn the_runbook_names_exactly_one_task_dispatch_command() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{
      "skill_name": "mr-review",
      "evals": [
        {"id": "one-shot", "prompt": "Fix it.", "expected_output": "fixed"},
        {"id": "scripted", "prompt": "Fix it.", "expected_output": "asks first",
         "turns": [{"prompt": "Use UTC.", "deliver_when": "always"}]}
      ]
    }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "codex", "--dry-run"])
        .assert()
        .success();

    let book = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    assert_eq!(
        book.matches("eval-magic dispatch --").count(),
        2,
        "one command for the eval tasks and one for the judges: {book}"
    );
    assert!(
        book.contains("eval-magic dispatch --judges"),
        "judges dispatch through the runner too: {book}"
    );
    assert_eq!(
        book.matches("eval-magic compare --").count(),
        2,
        "one comparison command per selected eval: {book}"
    );
    assert!(book.contains("--eval one-shot"), "{book}");
    assert!(book.contains("--eval scripted"), "{book}");
    for recipe_tool in ["xargs", "jq ", "tr -d"] {
        assert!(
            !book.contains(recipe_tool),
            "no pasted shell pipeline survives ({recipe_tool}): {book}"
        );
    }
    assert!(!book.contains("{{"), "no unsubstituted tokens: {book}");
}

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
        book.contains("eval-magic dispatch --skill-dir"),
        "carries the runner-driven dispatch command: {book}"
    );
    assert!(
        book.contains("--harness codex"),
        "pipeline commands carry --harness codex: {book}"
    );
    for guidance in [
        "same generated task command",
        "equivalent inputs and configuration",
        "fails inside the operator Codex session",
        "Operation not permitted",
        "alone does not establish",
        "Prefer running",
        "ordinary terminal",
        "outer launch of `eval-magic dispatch`",
        "surface and policy support",
        "--sandbox workspace-write",
        "eval guard enabled",
        "eval-magic docs isolation",
    ] {
        assert!(book.contains(guidance), "missing {guidance:?}: {book}");
    }
    assert!(
        book.find("**Codex inside Codex:**").unwrap()
            < book.find("\neval-magic dispatch --skill-dir").unwrap(),
        "the Codex note precedes the pasteable dispatch command: {book}"
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
    // Every harness now uses the same shared template: the runner drives the
    // dispatch, so the command differs only by its `--harness` selector. Each
    // task still runs in its own per-(group, condition) env, so the runbook
    // lives in the iteration dir, above those envs.
    assert!(
        book.contains("human driving"),
        "frames the run for a human at a terminal: {book}"
    );
    assert!(
        book.contains("--harness claude-code"),
        "pipeline commands carry --harness claude-code: {book}"
    );
    assert!(
        !book.contains("switch-condition"),
        "headless does not use the in-session batch loop: {book}"
    );
    assert!(
        !book.contains("Codex inside Codex") && !book.contains("Operation not permitted"),
        "other harnesses omit the Codex troubleshooting note: {book}"
    );
    assert!(!book.contains("{{"), "no unsubstituted tokens: {book}");

    // The runbook is the manual for a campaign, so it names the shell it
    // expects above the first command a reader would paste.
    let requirement = book
        .find("WSL")
        .expect("the runbook states the Windows-through-WSL requirement");
    assert!(!book.contains("Git Bash"), "{book}");
    // Anchored at a line start: the requirement prose names the command too,
    // and what this pins is the order of the *pasteable* line against it.
    assert!(
        requirement < book.find("\neval-magic dispatch --skill-dir").unwrap(),
        "the requirement precedes the first pasteable command: {book}"
    );
}

/// A prepared workspace remains correct when the host has no POSIX shell, so
/// `run` warns and names the required environment rather than failing.
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
        .stderr(contains("⚠").and(contains("WSL")))
        .stderr(contains("Git Bash").not());
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
        book.contains("--harness opencode"),
        "pipeline commands carry --harness opencode: {book}"
    );
    assert!(
        !book.contains("Codex inside Codex") && !book.contains("Operation not permitted"),
        "other harnesses omit the Codex troubleshooting note: {book}"
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
    // Dispatch shells out to POSIX command lines, so the manifest states the
    // same requirement as the runbook.
    assert!(
        manifest.contains("WSL") && !manifest.contains("Git Bash"),
        "the manifest states the POSIX shell requirement: {manifest}"
    );
}
