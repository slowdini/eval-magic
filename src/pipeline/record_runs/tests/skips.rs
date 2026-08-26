//! When the stage declines to write: an existing record or timing file it must
//! not clobber, and a task whose transcript is missing.

use super::*;

#[test]
fn skips_existing_run_without_overwrite_then_replaces_with_it() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "crash",
            condition: "with_skill",
        }],
    );
    write_claude_events(&paths[0].outputs_dir, "New.");
    let hand_written = json!({
        "eval_id": "crash", "condition": "with_skill",
        "skill_path": "/staged/skill/SKILL.md", "prompt": "Do the crash task",
        "files": [], "final_message": "Agent-authored.", "tool_invocations": []
    });
    fs::write(&paths[0].run_record_path, hand_written.to_string()).unwrap();

    let skipped = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();
    assert_eq!(skipped.recorded, 0);
    assert_eq!(skipped.skipped_existing, 1);
    assert_eq!(
        read_run(&iter, "crash", "with_skill").final_message,
        "Agent-authored."
    );

    let replaced = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), true).unwrap();
    assert_eq!(replaced.recorded, 1);
    assert_eq!(read_run(&iter, "crash", "with_skill").final_message, "New.");
}

#[test]
fn backfills_timing_only_when_absent() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "crash",
            condition: "with_skill",
        }],
    );
    write_claude_events(&paths[0].outputs_dir, "unused");
    fs::write(
        &paths[0].timing_path,
        json!({"total_tokens": 12345, "duration_ms": 9000}).to_string(),
    )
    .unwrap();

    record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();

    // Agent-captured completion-event timing wins; not overwritten.
    let timing = read_timing_value(&iter, "crash", "with_skill");
    assert_eq!(timing["total_tokens"], json!(12345));
    assert_eq!(timing["duration_ms"], json!(9000));
    assert!(timing.get("source").is_none());
}

#[test]
fn skips_the_run_when_its_transcript_is_missing() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "crash",
            condition: "with_skill",
        }],
    );
    // Completion metadata exists, but no transcript owns a final response.

    let result = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 0);
    assert_eq!(result.missing_transcript, 1);
    assert_eq!(result.skipped_no_final_response, 1);
    let warning = result
        .transcript_warning(Harness::resolve("claude-code").unwrap())
        .expect("missing transcript is reported");
    assert!(
        warning.contains("1 task missing transcript evidence"),
        "{warning}"
    );
    assert!(
        warning.contains("no final response was skipped"),
        "{warning}"
    );
    assert!(!warning.contains("one-shot runs lack"), "{warning}");
    assert!(!run_exists(&iter, "crash", "with_skill"));
    assert!(!timing_exists(&iter, "crash", "with_skill"));
}
