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
