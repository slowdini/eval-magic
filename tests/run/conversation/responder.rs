//! Conversations the LLM responder drives to an end.
//!
//! The guardrails around what it is shown and what it is allowed to say live
//! next door, in `responder_guards`; the stub and the scaffolding both files
//! share are here.
//!
//! Each test swaps the frozen descriptor's dispatch templates for a POSIX stub.
//! The same `exec_template` runs both the agent's first round and every
//! responder consultation, so the stub tells them apart the only way the runner
//! does: by the prompt file it is pointed at.

use super::{dispatch_one, stub_exec_template};
use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use std::path::{Path, PathBuf};

/// An evals config whose single eval is driven by the responder.
pub(super) fn responder_evals(max_turns: Option<u32>) -> String {
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
        "expected_output": "a working cache keyed on the pricing endpoint",
        "responder": {{ "type": "llm"{bound} }}
      }}]
    }}"#
    )
}

/// Prepare a responder-driven iteration against the codex harness.
pub(super) fn prepare(skill_dir: &Path, cwd: &Path) {
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
            "--responder-model",
            "test-responder-model",
        ])
        .assert()
        .success();
}

/// A stub standing in for both agents. Pointed at a responder prompt it copies
/// the canned verdict for that round into the consultation's output directory;
/// pointed at a task prompt it emits `$3` as the agent's message, plus the
/// session id and usage events a transcript needs to parse. Two canned bodies
/// are sentinels rather than verdicts: `EXIT-NONZERO` fails the dispatch, and
/// `NO-WRITE` succeeds while writing nothing.
fn stub(dir: &Path, name: &str) -> PathBuf {
    let script = dir.join(name);
    fs::write(
        &script,
        r#"#!/bin/sh
outputs=$1
prompt_path=$2
message=$3
verdicts=$4
case "$prompt_path" in
  */responder/*)
    round=$(basename "$outputs" | sed 's/^turn-//')
    file="$verdicts/$round.json"
    [ -f "$file" ] || file="$verdicts/default.json"
    case "$(cat "$file")" in
      EXIT-NONZERO) exit 3 ;;
      NO-WRITE) exit 0 ;;
    esac
    cat "$file" > "$outputs/verdict.json"
    exit 0
    ;;
esac
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

/// Wire the agent's two messages and a directory of canned verdicts into the
/// frozen descriptor. `verdicts` maps a consultation round to the verdict file
/// the responder "writes"; `default` covers every round without one.
pub(super) fn stub_rounds(
    tmp: &Path,
    cwd: &Path,
    initial: &str,
    resumed: &str,
    verdicts: &[(&str, &str)],
) -> PathBuf {
    let script = stub(tmp, "fake-codex.sh");
    let quoted = script.to_string_lossy().to_string();

    let verdict_dir = tmp.join("verdicts");
    fs::create_dir_all(&verdict_dir).unwrap();
    for (round, body) in verdicts {
        fs::write(verdict_dir.join(format!("{round}.json")), body).unwrap();
    }
    let verdict_dir_quoted = verdict_dir.to_string_lossy().to_string();

    stub_exec_template(
        cwd,
        &format!(
            "sh \"{quoted}\" <outputs_dir> <dispatch_prompt_path> \"{initial}\" \"{verdict_dir_quoted}\" <eval-root>"
        ),
    );
    let dispatch_path = iteration_dir(cwd).join("dispatch.json");
    let mut dispatch = read_json(&dispatch_path);
    dispatch["harness_descriptor"]["conversation"]["resume_exec_template"] = serde_json::json!(
        format!(
            "sh \"{quoted}\" <outputs_dir> <dispatch_prompt_path> \"{resumed}\" \"{verdict_dir_quoted}\" <eval-root> {{session_arg}} {{prompt_arg}}"
        )
    );
    fs::write(
        &dispatch_path,
        format!("{}\n", serde_json::to_string_pretty(&dispatch).unwrap()),
    )
    .unwrap();
    verdict_dir
}

pub(super) const ANSWER: &str = r#"{"verdict":"answer","reply":"An in-process LRU is fine.","rationale":"the simplest option that needs no new service"}"#;
pub(super) const DONE: &str =
    r#"{"verdict":"done","rationale":"the agent reported the cache in place and asked nothing"}"#;

pub(super) fn conversation_of(cwd: &Path, task: usize) -> serde_json::Value {
    let dispatch = read_json(&iteration_dir(cwd).join("dispatch.json"));
    let path = dispatch["tasks"][task]["conversation_path"]
        .as_str()
        .unwrap()
        .to_string();
    read_json(Path::new(&path))
}

/// The acceptance criterion from the ticket: a free-form question the old
/// heuristic could not classify is answered, and the run continues to
/// completion.
#[test]
fn a_free_form_question_is_answered_and_the_run_completes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);
    stub_rounds(
        tmp.path(),
        &cwd,
        "What should happen to rows with a null created_at?",
        "Caching is in place and the endpoint is under 40ms.",
        &[("1", ANSWER), ("2", DONE)],
    );

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();

    let conversation = conversation_of(&cwd, 0);
    assert_eq!(conversation["status"], "completed", "{conversation}");
    assert_eq!(conversation["delivered_followups"], 1);

    let synthesized = conversation["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "user_message")
        .nth(1)
        .expect("the responder delivered a second user turn");
    assert_eq!(synthesized["text"], "An in-process LRU is fine.");
    assert_eq!(synthesized["round"], 2);
    assert_eq!(synthesized["origin"]["responder"], "llm");
    assert_eq!(
        synthesized["origin"]["rationale"],
        "the simplest option that needs no new service"
    );
    assert_eq!(conversation["responder_outcome"]["ending"], "done");
}

/// The opening prompt is authored, not derived, so it carries no origin. The
/// absence is what lets a reader tell a real user turn from a synthesized one.
#[test]
fn the_opening_prompt_carries_no_responder_origin() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);
    stub_rounds(
        tmp.path(),
        &cwd,
        "Done, caching is in place.",
        "unused",
        &[("default", DONE)],
    );

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();

    let conversation = conversation_of(&cwd, 0);
    assert_eq!(conversation["status"], "completed", "{conversation}");
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
    let asking = "Which cache should I use?";
    stub_rounds(
        tmp.path(),
        &cwd,
        asking,
        asking,
        &[
            ("1", ANSWER),
            (
                "2",
                r#"{"verdict":"answer","reply":"Redis is fine too.","rationale":"still asking"}"#,
            ),
            (
                "3",
                r#"{"verdict":"answer","reply":"Whatever you prefer.","rationale":"still asking"}"#,
            ),
        ],
    );

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
        conversation["responder_outcome"].is_null(),
        "the bound is the runner's decision, not a verdict: {conversation}"
    );
    assert!(
        !Path::new(task["outputs_dir"].as_str().unwrap())
            .join("turn-4")
            .exists(),
        "the bound is the last round dispatched"
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

/// The responder is a second model in the attribution picture, so the run has
/// to say which one answered.
#[test]
fn the_responder_model_is_recorded_as_run_provenance() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &responder_evals(None));
    prepare(&skill_dir, &cwd);

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(conditions["responder_model"], "test-responder-model");

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(dispatch["responder_model"], "test-responder-model");
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
        "Which cache should I use?",
        "Caching is in place.",
        &[("1", ANSWER), ("2", DONE)],
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
        dispatch["tasks"][0]["responder"]["type"], "llm",
        "the plan records how the conversation was driven"
    );
}
