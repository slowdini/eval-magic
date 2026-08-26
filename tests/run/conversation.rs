//! Scripted multi-turn dispatch and harness capability checks.

use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::Value;
use std::fs;
use std::path::Path;

mod dispatch;
mod responder;
mod responder_guards;

#[test]
fn multi_turn_eval_dispatch_records_followups_and_conversation_artifact_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{
      "skill_name": "mr-review",
      "evals": [{
        "id": "clarify",
        "prompt": "The date is wrong. Fix it.",
        "expected_output": "asks before editing",
        "turns": [
          {
            "prompt": "Affected users are in US timezones.",
            "deliver_when": "agent_asks",
            "agent_response_matches": "(?i)time ?zone"
          },
          {
            "prompt": "It is a date-only field.",
            "deliver_when": "always"
          }
        ]
      }]
    }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);

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
            "codex",
            "--agent-model",
            "test-agent",
            "--no-guard",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(dispatch["harness_descriptor"]["label"], "codex");
    assert_eq!(dispatch["agent_model"], "test-agent");
    for task in dispatch["tasks"].as_array().unwrap() {
        assert_eq!(task["turns"].as_array().unwrap().len(), 2);
        assert!(
            task["conversation_path"]
                .as_str()
                .unwrap()
                .ends_with("/conversation.json")
        );
    }
}

/// The runner drives every task, so the frozen descriptor and guard state the
/// driver needs must reach `dispatch.json` even when no eval declares `turns`.
/// Gating them on scripted turns left a one-shot plan undispatchable.
#[test]
fn a_one_shot_only_plan_still_carries_the_descriptor_the_driver_needs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{
      "skill_name": "mr-review",
      "evals": [{
        "id": "one-shot",
        "prompt": "Fix the date.",
        "expected_output": "fixed"
      }]
    }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);

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
            "codex",
            "--no-guard",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(
        dispatch["harness_descriptor"]["label"], "codex",
        "a one-shot plan needs the frozen descriptor: {dispatch}"
    );
    assert_eq!(dispatch["guard"], false);
    for task in dispatch["tasks"].as_array().unwrap() {
        assert!(task["turns"].is_null(), "this plan declares no turns");
        assert!(
            task["conversation_path"]
                .as_str()
                .unwrap()
                .ends_with("/conversation.json")
        );
    }
}

/// A task with no scripted turns is driven by the runner too, and leaves the
/// same evidence a scripted one does: a turn-1 transcript and a completion
/// artifact recording zero delivered follow-ups.
#[test]
fn the_driver_runs_a_task_that_declares_no_scripted_turns() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{
      "skill_name": "mr-review",
      "evals": [{
        "id": "one-shot",
        "prompt": "Fix the date.",
        "expected_output": "fixed"
      }]
    }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);

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
            "codex",
            "--no-guard",
        ])
        .assert()
        .success();

    let dispatch_path = iteration_dir(&cwd).join("dispatch.json");
    let mut dispatch = read_json(&dispatch_path);
    dispatch["harness_descriptor"]["dispatch"]["exec_template"] =
        serde_json::json!(one_shot_stub(tmp.path(), "Updated the date handling."));
    fs::write(
        &dispatch_path,
        format!("{}\n", serde_json::to_string_pretty(&dispatch).unwrap()),
    )
    .unwrap();

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();

    let task = &dispatch["tasks"][0];
    let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));
    assert_eq!(conversation["status"], "completed");
    assert_eq!(conversation["delivered_followups"], 0);
    assert_eq!(
        conversation["events"],
        serde_json::json!([{
            "type": "user_message",
            "ordinal": 0,
            "round": 1,
            "text": "Fix the date."
        }])
    );

    let outputs = Path::new(task["outputs_dir"].as_str().unwrap());
    assert!(
        outputs.join("turn-1").join("codex-events.jsonl").is_file(),
        "a one-shot transcript belongs under turn-1 like every other round"
    );
    assert!(!outputs.join("final-message.md").exists());
}

const ONE_SHOT_EVALS: &str = r#"{
  "skill_name": "mr-review",
  "evals": [{
    "id": "one-shot",
    "prompt": "Fix the date.",
    "expected_output": "fixed"
  }]
}"#;

/// Prepare an iteration with no scripted turns and no guard.
fn prepare_one_shot_run(skill_dir: &Path, cwd: &Path, harness: &str) {
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
            harness,
            "--no-guard",
        ])
        .assert()
        .success();
}

/// Drive one task of a prepared iteration through `dispatch`. Returns the
/// command so a caller can assert success or failure.
fn dispatch_one(
    skill_dir: &Path,
    cwd: &Path,
    harness: &str,
    index: usize,
    overwrite: bool,
) -> assert_cmd::Command {
    let mut command = skill_eval();
    command
        .current_dir(cwd)
        .args(["dispatch", "--skill-dir"])
        .arg(skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            harness,
            "--task-index",
            &index.to_string(),
        ]);
    if overwrite {
        command.arg("--overwrite");
    }
    command
}

/// Swap the frozen descriptor's `exec_template` for a stub, the way every
/// driver test here substitutes a real harness CLI.
fn stub_exec_template(cwd: &Path, template: &str) {
    let dispatch_path = iteration_dir(cwd).join("dispatch.json");
    let mut dispatch = read_json(&dispatch_path);
    dispatch["harness_descriptor"]["dispatch"]["exec_template"] = serde_json::json!(template);
    fs::write(
        &dispatch_path,
        format!("{}\n", serde_json::to_string_pretty(&dispatch).unwrap()),
    )
    .unwrap();
}

/// A one-shot harness stub: emits one agent message and a usage event without a
/// resumable session id. Written as a POSIX
/// script and invoked through `sh` for the same reason the scripted stub below
/// is — that is the shape of a real `exec_template`, and it needs no executable
/// bit on any host.
fn one_shot_stub(dir: &Path, message: &str) -> String {
    let script = dir.join("fake-one-shot.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
outputs=$1
message=$2
printf '%s\n' "{\"type\":\"item.completed\",\"item\":{\"id\":\"m1\",\"type\":\"agent_message\",\"text\":\"$message\"}}" > "$outputs/codex-events.jsonl"
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}' >> "$outputs/codex-events.jsonl"
"#,
    )
    .unwrap();
    format!(
        "sh \"{}\" <outputs_dir> \"{message}\"",
        script.to_string_lossy()
    )
}

// The harness stub below is a POSIX shell script, because that is what a real `exec_template` is —
// every descriptor in `harnesses/` ships one (`</dev/null`, `\` continuations, `>` redirection).
// The driver resolves an `sh` on every host, and the template invokes the stub *through* that `sh`
// rather than executing it directly, so no executable bit is involved and the scripted-turn path is
// covered everywhere.
#[test]
fn dispatch_task_runs_all_scripted_turns_in_one_native_session() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{
      "skill_name": "mr-review",
      "evals": [{
        "id": "clarify",
        "prompt": "Fix the date.",
        "expected_output": "asks before editing",
        "turns": [
          {
            "prompt": "Affected users are in US timezones.",
            "deliver_when": "agent_asks",
            "agent_response_matches": "(?i)time ?zone"
          },
          {
            "prompt": "It is a date-only field.",
            "deliver_when": "always"
          }
        ]
      }]
    }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);

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
            "codex",
            "--no-guard",
            "--agent-env",
            "ROUND_ENV=visible",
        ])
        .assert()
        .success();

    let fixture = tmp.path().join("fake-codex.sh");
    fs::write(
        &fixture,
        r#"#!/bin/sh
outputs=$1
mode=$2
if [ "$ROUND_ENV" != "visible" ]; then
  exit 43
fi
if [ "$mode" != "initial" ] && [ "$4" != "session-1" ]; then
  exit 42
fi
printf '%s\n' '{"type":"thread.started","thread_id":"session-1"}' > "$outputs/codex-events.jsonl"
if [ "$mode" = "initial" ]; then
  case "$3" in
    *without_skill*) message="Which locale should I use?" ;;
    *) message="Which timezone should I use?" ;;
  esac
  printf '%s\n' "{\"type\":\"item.completed\",\"item\":{\"id\":\"m1\",\"type\":\"agent_message\",\"text\":\"$message\"}}" >> "$outputs/codex-events.jsonl"
elif [ "$mode" = "resume-1" ]; then
  printf '%s\n' '{"type":"item.completed","item":{"id":"m2","type":"agent_message","text":"Thanks. Is this a date-only field?"}}' >> "$outputs/codex-events.jsonl"
else
  printf '%s\n' '{"type":"item.completed","item":{"id":"m3","type":"agent_message","text":"Updated the date handling."}}' >> "$outputs/codex-events.jsonl"
fi
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}' >> "$outputs/codex-events.jsonl"
"#,
    )
    .unwrap();

    let dispatch_path = iteration_dir(&cwd).join("dispatch.json");
    let mut dispatch = read_json(&dispatch_path);
    // `sh <stub>` rather than `<stub>`: the script needs no executable bit that
    // way, and the shell running it is the one the driver already resolved.
    let stub = format!("sh \"{}\"", fixture.to_string_lossy());
    dispatch["harness_descriptor"]["dispatch"]["exec_template"] =
        serde_json::json!(format!("{stub} <outputs_dir> initial <eval-root>"));
    dispatch["harness_descriptor"]["conversation"]["resume_exec_template"] = serde_json::json!(
        format!("{stub} <outputs_dir> resume-<round> <eval-root> {{session_arg}} {{prompt_arg}}")
    );
    fs::write(
        &dispatch_path,
        format!("{}\n", serde_json::to_string_pretty(&dispatch).unwrap()),
    )
    .unwrap();

    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();

    let task = &dispatch["tasks"][0];
    let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));
    assert_eq!(conversation["status"], "completed");
    assert_eq!(conversation["delivered_followups"], 2);
    assert_eq!(conversation["stop_reason"], Value::Null);
    assert_eq!(
        conversation["events"],
        serde_json::json!([
            {
                "type": "user_message",
                "ordinal": 0,
                "round": 1,
                "text": "Fix the date."
            },
            {
                "type": "user_message",
                "ordinal": 1,
                "round": 2,
                "text": "Affected users are in US timezones."
            },
            {
                "type": "user_message",
                "ordinal": 2,
                "round": 3,
                "text": "It is a date-only field."
            }
        ])
    );
    assert!(
        !Path::new(task["outputs_dir"].as_str().unwrap())
            .join("final-message.md")
            .exists(),
        "the final assistant response comes from the last round transcript"
    );

    dispatch_one(&skill_dir, &cwd, "codex", 1, false)
        .assert()
        .success();

    let stopped_task = &dispatch["tasks"][1];
    let stopped = read_json(Path::new(
        stopped_task["conversation_path"].as_str().unwrap(),
    ));
    assert_eq!(stopped["status"], "stopped");
    assert_eq!(stopped["delivered_followups"], 0);
    assert_eq!(stopped["stop_reason"], "agent_response_mismatch");
    assert_eq!(stopped["stopped_before_followup"], 1);
    assert!(
        !Path::new(stopped_task["outputs_dir"].as_str().unwrap())
            .join("turn-2")
            .exists(),
        "an unmet compatibility gate must not deliver the canned reply"
    );

    let completed_path =
        Path::new(dispatch["tasks"][0]["conversation_path"].as_str().unwrap()).to_path_buf();
    dispatch["harness_descriptor"]["dispatch"]["exec_template"] =
        serde_json::json!("false <eval-root> <outputs_dir>");
    fs::write(
        &dispatch_path,
        format!("{}\n", serde_json::to_string_pretty(&dispatch).unwrap()),
    )
    .unwrap();

    dispatch_one(&skill_dir, &cwd, "codex", 0, true)
        .assert()
        .failure();
    assert!(
        !completed_path.exists(),
        "an interrupted overwrite must not leave a stale completion artifact"
    );
}

#[test]
fn multi_turn_eval_rejects_a_harness_without_native_resume_support() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{
      "skill_name": "mr-review",
      "evals": [{
        "id": "clarify",
        "prompt": "Fix the date.",
        "expected_output": "asks a question",
        "turns": [{"prompt": "Use US timezones.", "deliver_when": "agent_asks"}]
      }]
    }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    let descriptor_dir = cwd.join(".eval-magic").join("harnesses");
    fs::create_dir_all(&descriptor_dir).unwrap();
    fs::write(
        descriptor_dir.join("cool.toml"),
        RUNNER_READY_NO_RESUME_DESCRIPTOR,
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
            contains("scripted follow-up turns")
                .and(contains("cool-custom-harness"))
                .and(contains("conversation")),
        );
}
