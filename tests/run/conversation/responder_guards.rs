//! What the responder is shown, and what it is never allowed to deliver.
//!
//! The scaffolding is `responder`'s: these tests drive the same stub, and vary
//! only the verdict it writes.

use super::dispatch_one;
use super::responder::{ANSWER, DONE, conversation_of, prepare, responder_evals, stub_rounds};
use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use std::path::Path;

/// The responder may only tell the agent what the agent already knows. The
/// eval's `expected_output` is the grading criterion, and a responder that had
/// read it could hand the agent the rubric.
#[test]
fn the_consultation_prompt_withholds_the_grading_criteria() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);
    stub_rounds(
        tmp.path(),
        &cwd,
        "Which cache should I use?",
        "Caching is in place.",
        &[("1", ANSWER), ("2", DONE)],
    );

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let responder_dir = dispatch["tasks"][0]["responder_dir"]
        .as_str()
        .expect("a responder task records where its consultations live")
        .to_string();
    let prompt = fs::read_to_string(Path::new(&responder_dir).join("turn-1").join("prompt.txt"))
        .expect("the first consultation wrote its prompt");

    assert!(prompt.contains("Requests to the pricing API are slow."));
    assert!(prompt.contains("Which cache should I use?"));
    assert!(
        !prompt.contains("a working cache keyed on the pricing endpoint"),
        "the grading criterion must not reach the responder: {prompt}"
    );
}

/// A responder that honestly cannot answer stops the run rather than inventing
/// a reply, and the cause distinguishes the refusal from a broken dispatch.
#[test]
fn a_declined_question_stops_the_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);
    stub_rounds(
        tmp.path(),
        &cwd,
        "Which production credential should I use?",
        "unused",
        &[(
            "default",
            r#"{"verdict":"cannot_answer","rationale":"it asked for a credential I was never given"}"#,
        )],
    );

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success()
        .stderr(contains("could not answer").and(contains("declined")));

    let conversation = conversation_of(&cwd, 0);
    assert_eq!(conversation["status"], "stopped", "{conversation}");
    assert_eq!(conversation["stop_reason"], "responder_cannot_answer");
    assert_eq!(conversation["responder_outcome"]["cause"], "declined");
    assert_eq!(conversation["delivered_followups"], 0);
    assert_eq!(conversation["stopped_before_followup"], 1);
}

/// A consultation that fails is a stop, not a failed run: `dispatch` still
/// exits zero and the artifact is still written, so the campaign keeps going
/// and the cause says what broke.
#[test]
fn a_failed_consultation_stops_the_run_without_failing_the_dispatch() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);
    stub_rounds(
        tmp.path(),
        &cwd,
        "Which cache should I use?",
        "unused",
        &[("default", "EXIT-NONZERO")],
    );

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success()
        .stdout(contains("1 stopped"))
        .stderr(contains("dispatch_failed"));

    let conversation = conversation_of(&cwd, 0);
    assert_eq!(conversation["status"], "stopped", "{conversation}");
    assert_eq!(conversation["stop_reason"], "responder_cannot_answer");
    assert_eq!(
        conversation["responder_outcome"]["cause"],
        "dispatch_failed"
    );
}

/// A dispatch that succeeds but writes nothing leaves no reply to deliver. It
/// stops for the same reason a refusal does, with its own cause.
#[test]
fn a_consultation_that_writes_no_verdict_stops_the_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);
    stub_rounds(
        tmp.path(),
        &cwd,
        "Which cache should I use?",
        "unused",
        &[("default", "")],
    );

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();

    let conversation = conversation_of(&cwd, 0);
    assert_eq!(conversation["stop_reason"], "responder_cannot_answer");
    assert_eq!(
        conversation["responder_outcome"]["cause"],
        "missing_verdict"
    );
}

/// A reply that fails validation is never delivered. The run stops loudly
/// rather than putting the responder's own work into the transcript.
#[test]
fn a_reply_carrying_code_is_never_delivered() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);
    stub_rounds(
        tmp.path(),
        &cwd,
        "Which cache should I use?",
        "unused",
        &[(
            "default",
            r#"{"verdict":"answer","reply":"Use this:\n\n```rust\nlet c = Lru::new(128);\n```\n"}"#,
        )],
    );

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = &dispatch["tasks"][0];
    let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));

    assert_eq!(conversation["stop_reason"], "responder_cannot_answer");
    assert_eq!(
        conversation["responder_outcome"]["cause"],
        "reply_contains_code"
    );
    assert_eq!(conversation["delivered_followups"], 0);
    assert!(
        !Path::new(task["outputs_dir"].as_str().unwrap())
            .join("turn-2")
            .exists(),
        "a rejected reply delivers no turn"
    );
}

/// A rerun must consult afresh. Reusing the verdict a previous dispatch left
/// on disk would answer this run's agent with a reply written about a different
/// conversation — the silent contamination the responder exists to avoid.
#[test]
fn a_rerun_does_not_reuse_the_previous_dispatch_verdict() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);
    let verdicts = stub_rounds(
        tmp.path(),
        &cwd,
        "Which cache should I use?",
        "Caching is in place.",
        &[("1", ANSWER), ("2", DONE)],
    );

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();
    assert_eq!(conversation_of(&cwd, 0)["status"], "completed");

    // The responder now writes nothing at all. Its previous verdict is still on
    // disk, and must not be read as this run's answer.
    fs::write(verdicts.join("1.json"), "NO-WRITE").unwrap();
    fs::write(verdicts.join("2.json"), "NO-WRITE").unwrap();

    dispatch_one(&skill_dir, &cwd, "codex", 0, true)
        .assert()
        .success();

    let conversation = conversation_of(&cwd, 0);
    assert_eq!(conversation["status"], "stopped", "{conversation}");
    assert_eq!(
        conversation["responder_outcome"]["cause"], "missing_verdict",
        "{conversation}"
    );
    assert_eq!(conversation["delivered_followups"], 0);
}
