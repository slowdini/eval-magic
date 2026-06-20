//! `RUNBOOK.md` generation during `run`: the followable isolated-session handoff
//! artifact, and the post-run pointer at it.

use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

#[test]
fn run_writes_interactive_runbook_and_points_at_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // A real run (not --dry-run) so the post-run "Next:" handoff prints; --dry-run
    // stops before next steps by contract.
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success()
        // The summary hands off to a fresh isolated session: cd into env/, then
        // "Read and follow RUNBOOK.md". It must not re-print the dispatch loop —
        // that lives only in RUNBOOK.md now (the session-juggling apparatus is gone).
        // (The exact env path in the handoff is locked by the util.rs unit test;
        // here we just confirm the handoff is wired into stdout.)
        .stdout(contains("Read and follow RUNBOOK.md"))
        .stdout(contains("1. cd "))
        .stdout(contains("one batch at a time").not());

    // The runbook lives inside the isolated env — the session's cwd reads it.
    assert!(!iteration_dir(&cwd).join("RUNBOOK.md").exists());
    let book = read_str(&env_dir(&cwd).join("RUNBOOK.md"));
    assert!(book.contains("mr-review"), "names the skill: {book}");
    assert!(
        book.contains("with_skill") && book.contains("without_skill"),
        "names both conditions: {book}"
    );
    assert!(
        book.contains("agent_description"),
        "carries the in-session dispatch guidance: {book}"
    );
    // The per-condition batch loop: a switch-condition barrier between the two
    // batches, carrying the absolute --workspace-dir so it resolves from env/.
    assert!(
        book.contains("eval-magic switch-condition --skill-dir")
            && book.contains("--workspace-dir")
            && book.contains("--condition without_skill"),
        "carries the switch-condition barrier between batches: {book}"
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

    assert!(!iteration_dir(&cwd).join("RUNBOOK.md").exists());
    let book = read_str(&env_dir(&cwd).join("RUNBOOK.md"));
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
