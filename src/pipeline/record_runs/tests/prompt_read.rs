//! The prompt-read guard (issue #109): whether a dispatch is recorded, judged
//! by what its transcript shows the agent's read of the dispatch prompt returned.

use super::*;

#[test]
fn flags_dispatch_whose_prompt_read_failed() {
    // A dispatch that couldn't read its prompt still exits 0 and emits a
    // final message — but the run is a silent no-op, not data (issue #109).
    let tmp = TempDir::new().unwrap();
    let iter = tmp.path();
    let paths = write_iteration(
        iter,
        &[FixtureTask {
            eval_id: "e1",
            condition: "with_skill",
            final_message: Some("I could not read the prompt file."),
        }],
    );
    let prompt_path = iter
        .join("eval-e1")
        .join("with_skill")
        .join("dispatch-prompt.txt");
    fs::write(
        &prompt_path,
        format!("{PROMPT_SENTINEL}\n\nUser request:\ndo it"),
    )
    .unwrap();
    // The transcript shows a Read of the prompt path that ERRORED — the
    // result is a denial, not the prompt content.
    write_claude_events_prompt_read(
        &paths[0].outputs_dir,
        &prompt_path.to_string_lossy(),
        "<tool_use_error>File is outside the allowed working directory.</tool_use_error>",
        "I could not read the prompt file.",
    );

    let result = record_runs(iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();

    assert_eq!(result.skipped_prompt_unread, 1);
    assert_eq!(result.recorded, 0);
    assert!(!paths[0].run_record_path.exists());
}

#[test]
fn records_dispatch_when_prompt_read_succeeded() {
    // The same shape, but the Read returned the prompt content (Read echoes
    // it with a line-number prefix) — a legitimate run, recorded as data.
    let tmp = TempDir::new().unwrap();
    let iter = tmp.path();
    let paths = write_iteration(
        iter,
        &[FixtureTask {
            eval_id: "e1",
            condition: "with_skill",
            final_message: Some("Done."),
        }],
    );
    let prompt_path = iter
        .join("eval-e1")
        .join("with_skill")
        .join("dispatch-prompt.txt");
    fs::write(
        &prompt_path,
        format!("{PROMPT_SENTINEL}\n\nUser request:\ndo it"),
    )
    .unwrap();
    write_claude_events_prompt_read(
        &paths[0].outputs_dir,
        &prompt_path.to_string_lossy(),
        &format!("     1→{PROMPT_SENTINEL}\n     2→\n     3→User request:"),
        "Done.",
    );

    let result = record_runs(iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();

    assert_eq!(result.recorded, 1);
    assert_eq!(result.skipped_prompt_unread, 0);
}

#[test]
fn records_dispatch_when_prompt_read_has_no_result_evidence() {
    // Declarative extract tiers can leave tool results unjoined (cline's
    // `content_start` events carry args but no result — the result lives in a
    // keyed `content_end` the extract tier cannot join). A result-less read is
    // not evidence of failure: the guard must stay silent rather than skip
    // every run (observed as a false positive on the cline smoke eval, where
    // both arms read the prompt successfully and were still flagged).
    let tmp = TempDir::new().unwrap();
    let iter = tmp.path();
    let paths = write_iteration(
        iter,
        &[FixtureTask {
            eval_id: "e1",
            condition: "with_skill",
            final_message: Some("Done."),
        }],
    );
    let prompt_path = iter
        .join("eval-e1")
        .join("with_skill")
        .join("dispatch-prompt.txt");
    fs::write(
        &prompt_path,
        format!("{PROMPT_SENTINEL}\n\nUser request:\ndo it"),
    )
    .unwrap();
    // A `cline --json` stream: the read's (nested) args reference the prompt
    // path, but the extract tier attaches no result to the invocation.
    let lines = vec![
        json!({"ts": "2026-08-11T19:29:50.000Z", "type": "agent_event", "event": {"type": "content_start", "contentType": "tool", "toolCallId": "read_files_0", "toolName": "read_files", "input": {"files": [{"path": prompt_path.to_string_lossy()}]}}}),
        json!({"ts": "2026-08-11T19:29:52.000Z", "type": "agent_event", "event": {"type": "content_end", "contentType": "text", "text": "Done."}}),
        json!({"ts": "2026-08-11T19:29:54.000Z", "type": "run_result", "finishReason": "completed", "iterations": 1, "usage": {"inputTokens": 10, "outputTokens": 2, "cacheReadTokens": 0, "cacheWriteTokens": 0, "totalCost": 0.001}, "durationMs": 4000, "text": "Done."}),
    ];
    fs::write(
        paths[0].outputs_dir.join("cline-events.jsonl"),
        jsonl(&lines),
    )
    .unwrap();

    let result = record_runs(iter, 1, Harness::resolve("cline").unwrap(), false).unwrap();

    assert_eq!(result.recorded, 1);
    assert_eq!(result.skipped_prompt_unread, 0);
}
