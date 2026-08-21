//! Conversations driven by the heuristic responder rather than a script.
//!
//! Each test swaps the frozen descriptor's dispatch templates for a POSIX stub
//! that answers differently per round, the way every driver test here does.

use super::{dispatch_one, stub_exec_template};
use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use std::path::{Path, PathBuf};

/// An evals config whose single eval is driven by the responder.
fn responder_evals(max_turns: Option<u32>) -> String {
    let bound = match max_turns {
        Some(turns) => format!(", \"max_turns\": {turns}"),
        None => String::new(),
    };
    format!(
        r#"{{
      "skill_name": "mr-review",
      "evals": [{{
        "id": "caching",
        "prompt": "Requests to the pricing API are slow. Add caching.",
        "expected_output": "caching is in place",
        "responder": {{ "type": "heuristic"{bound} }}
      }}]
    }}"#
    )
}

/// Prepare a responder-driven iteration against the codex harness.
fn prepare(skill_dir: &Path, cwd: &Path) {
    skill_eval()
        .current_dir(cwd)
        .args(["run", "--skill-dir"])
        .arg(skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--harness",
            "codex",
            "--no-guard",
        ])
        .assert()
        .success();
}

/// A stub emitting `$2` as its agent message for every round, plus the session
/// id and usage events a transcript needs to parse. Written as a POSIX script
/// and invoked through `sh`, because that is the shape of a real exec template.
fn stub(dir: &Path, name: &str) -> PathBuf {
    let script = dir.join(name);
    fs::write(
        &script,
        r#"#!/bin/sh
outputs=$1
message=$2
printf '%s\n' '{"type":"thread.started","thread_id":"session-1"}' > "$outputs/codex-events.jsonl"
printf '%s' '{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"' >> "$outputs/codex-events.jsonl"
printf '%s' "$message" >> "$outputs/codex-events.jsonl"
printf '%s\n' '"}}' >> "$outputs/codex-events.jsonl"
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}' >> "$outputs/codex-events.jsonl"
"#,
    )
    .unwrap();
    script
}

/// Wire an initial message and a resume message into the frozen descriptor.
fn stub_rounds(tmp: &Path, cwd: &Path, initial: &str, resumed: &str) {
    let script = stub(tmp, "fake-codex.sh");
    let quoted = script.to_string_lossy().to_string();
    stub_exec_template(
        cwd,
        &format!("sh \"{quoted}\" <outputs_dir> \"{initial}\" <eval-root>"),
    );
    let dispatch_path = iteration_dir(cwd).join("dispatch.json");
    let mut dispatch = read_json(&dispatch_path);
    dispatch["harness_descriptor"]["conversation"]["resume_exec_template"] =
        serde_json::json!(format!(
            "sh \"{quoted}\" <outputs_dir> \"{resumed}\" <eval-root> {{session_arg}} {{prompt_arg}}"
        ));
    fs::write(
        &dispatch_path,
        format!("{}\n", serde_json::to_string_pretty(&dispatch).unwrap()),
    )
    .unwrap();
}

/// The acceptance criterion from the ticket: a responder eval with no scripted
/// turns runs to completion, and the recommended option is both selected and
/// recorded as the reason it was selected.
#[test]
fn a_responder_eval_answers_a_recommended_option_and_completes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);
    stub_rounds(
        tmp.path(),
        &cwd,
        "Which cache should I use?\\n\\n- In-process LRU (Recommended)\\n- Redis\\n",
        "Caching is in place and the endpoint is under 40ms.",
    );

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = &dispatch["tasks"][0];
    let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));

    assert_eq!(conversation["status"], "completed", "{conversation}");
    assert_eq!(conversation["delivered_followups"], 1);
    let synthesized = conversation["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "user_message")
        .nth(1)
        .expect("the responder delivered a second user turn");
    assert_eq!(synthesized["text"], "In-process LRU");
    assert_eq!(synthesized["round"], 2);
    assert_eq!(synthesized["origin"]["responder"], "heuristic");
    assert_eq!(
        synthesized["origin"]["answers"][0]["rule"],
        "recommended_option"
    );
    assert_eq!(
        synthesized["origin"]["answers"][0]["question"],
        "Which cache should I use?"
    );
}

/// The opening prompt is authored, not derived, so it carries no origin. The
/// absence is what lets a reader tell a real user turn from a synthesized one.
#[test]
fn the_opening_prompt_carries_no_responder_origin() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);
    stub_rounds(tmp.path(), &cwd, "Done, caching is in place.", "unused");

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let conversation = read_json(Path::new(
        dispatch["tasks"][0]["conversation_path"].as_str().unwrap(),
    ));
    assert_eq!(conversation["status"], "completed");
    assert_eq!(conversation["delivered_followups"], 0);
    assert!(
        conversation["events"][0]["origin"].is_null(),
        "the eval prompt is authored: {conversation}"
    );
}

/// Reaching the bound is a recorded outcome, not a failure: the command still
/// exits zero and the artifact says exactly why the conversation ended.
#[test]
fn a_responder_run_that_reaches_max_turns_is_recorded_not_failed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(Some(2)));
    prepare(&skill_dir, &cwd);
    let asking = "Which cache should I use?\\n\\n- In-process LRU (Recommended)\\n- Redis\\n";
    stub_rounds(tmp.path(), &cwd, asking, asking);

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = &dispatch["tasks"][0];
    let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));

    assert_eq!(conversation["status"], "stopped", "{conversation}");
    assert_eq!(conversation["stop_reason"], "max_turns_reached");
    assert_eq!(conversation["delivered_followups"], 2);
    assert_eq!(conversation["stopped_before_followup"], 3);
    assert!(
        !Path::new(task["outputs_dir"].as_str().unwrap())
            .join("turn-4")
            .exists(),
        "the bound is the last round dispatched"
    );
}

/// The greppable branch the LLM responder will take over. It stops the run
/// rather than inventing an answer, and says so loudly — a conversation that
/// ended mid-task must not be mistaken for a clean data point.
#[test]
fn a_question_the_responder_cannot_classify_stops_the_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);
    stub_rounds(
        tmp.path(),
        &cwd,
        "What should happen to rows with a null created_at?",
        "unused",
    );

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success()
        .stderr(contains("could not answer"));

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = &dispatch["tasks"][0];
    let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));

    assert_eq!(conversation["status"], "stopped", "{conversation}");
    assert_eq!(conversation["stop_reason"], "responder_cannot_answer");
    assert_eq!(conversation["delivered_followups"], 0);
    assert_eq!(conversation["stopped_before_followup"], 1);
    assert!(
        !Path::new(task["outputs_dir"].as_str().unwrap())
            .join("turn-2")
            .exists(),
        "an unanswerable question delivers no turn"
    );
}

/// A responder needs the same native-resume capability a scripted array does:
/// starting a fresh session each round would make the answer meaningless. `run`
/// has to say so at prep time, not leave it to fail mid-dispatch.
#[test]
fn a_responder_eval_is_rejected_on_a_harness_without_native_resume() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    let descriptor_dir = cwd.join(".eval-magic").join("harnesses");
    fs::create_dir_all(&descriptor_dir).unwrap();
    fs::write(
        descriptor_dir.join("cool.toml"),
        r#"label = "cool-custom-harness"

[dispatch]
exec_template = "cool-cli run --cd <eval-root>{model_arg} <dispatch_prompt_path> > <outputs_dir>/final-message.md"
"#,
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--harness",
            "cool-custom-harness",
        ])
        .assert()
        .failure()
        .stderr(
            contains("responder")
                .and(contains("cool-custom-harness"))
                .and(contains("conversation")),
        );
}

/// Mode B parity: a revision run drives a responder conversation the same way a
/// new-skill run does, against the snapshot/promote path.
#[test]
fn revision_mode_runs_a_responder_eval() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));

    skill_eval()
        .current_dir(&cwd)
        .args(["snapshot", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--label", "baseline"])
        .assert()
        .success();
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "revision",
            "--harness",
            "codex",
            "--no-guard",
        ])
        .assert()
        .success();
    stub_rounds(
        tmp.path(),
        &cwd,
        "Which cache should I use?\\n\\n- In-process LRU (Recommended)\\n- Redis\\n",
        "Caching is in place.",
    );

    skill_eval()
        .current_dir(&cwd)
        .args(["dispatch", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "codex",
        ])
        .assert()
        .success()
        .stdout(contains("2 completed"));

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));
        assert_eq!(conversation["status"], "completed", "{conversation}");
        assert_eq!(conversation["delivered_followups"], 1);
    }
    assert_eq!(
        dispatch["tasks"][0]["responder"]["type"], "heuristic",
        "the plan records how the conversation was driven"
    );
}
