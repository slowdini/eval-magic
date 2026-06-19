//! `RUNBOOK.md` generation during `run`: the followable isolated-session handoff
//! artifact, and the post-run pointer at it.

use crate::helpers::*;
use predicates::str::contains;

#[test]
fn run_writes_interactive_runbook_and_points_at_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("Read and follow RUNBOOK.md"));

    let book = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    assert!(book.contains("mr-review"), "names the skill: {book}");
    assert!(
        book.contains("with_skill") && book.contains("without_skill"),
        "names both conditions: {book}"
    );
    assert!(
        book.contains("agent_description"),
        "carries the in-session dispatch guidance: {book}"
    );
    assert!(
        book.contains("eval-magic ingest --skill-dir"),
        "carries the ingest command: {book}"
    );
    assert!(
        book.contains("eval-magic finalize --skill-dir"),
        "carries the finalize command: {book}"
    );
    assert!(
        book.contains("benchmark.json"),
        "points at the result: {book}"
    );
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

    let book = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    assert!(
        book.contains("human driving"),
        "frames the run for a human at a terminal: {book}"
    );
    assert!(
        book.contains("codex exec"),
        "carries the Codex CLI dispatch recipe: {book}"
    );
    assert!(
        book.contains("--harness codex"),
        "pipeline commands carry --harness codex: {book}"
    );
    assert!(!book.contains("{{"), "no unsubstituted tokens: {book}");
}
