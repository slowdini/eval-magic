//! Scripted multi-turn dispatch and harness capability checks.

use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
// Every `#[cfg(unix)]` import below exists solely for
// `dispatch_task_runs_all_scripted_turns_in_one_native_session` — see the note on that test.
#[cfg(unix)]
use serde_json::Value;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::Path;

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

// The harness stub below is a `#!/bin/sh` script invoked directly through the descriptor's
// exec_template, which needs both a POSIX shell and an executable bit. Windows has neither, so the
// scripted-turn path is covered on Unix only until a portable stub replaces it.
#[cfg(unix)]
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
    let mut permissions = fs::metadata(&fixture).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fixture, permissions).unwrap();

    let dispatch_path = iteration_dir(&cwd).join("dispatch.json");
    let mut dispatch = read_json(&dispatch_path);
    let fixture = fixture.to_string_lossy();
    dispatch["harness_descriptor"]["dispatch"]["exec_template"] =
        serde_json::json!(format!("{fixture} <outputs_dir> initial <eval-root>"));
    dispatch["harness_descriptor"]["conversation"]["resume_exec_template"] =
        serde_json::json!(format!(
            "{fixture} <outputs_dir> resume-<round> <eval-root> {{session_arg}} {{prompt_arg}}"
        ));
    fs::write(
        &dispatch_path,
        format!("{}\n", serde_json::to_string_pretty(&dispatch).unwrap()),
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["dispatch-task", "--dispatch"])
        .arg(&dispatch_path)
        .args(["--task-index", "0"])
        .assert()
        .success();

    let task = &dispatch["tasks"][0];
    let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));
    assert_eq!(conversation["status"], "completed");
    assert_eq!(conversation["delivered_followups"], 2);
    assert_eq!(conversation["stop_reason"], Value::Null);
    assert_eq!(
        conversation["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["type"] == "user_message")
            .count(),
        3
    );
    assert_eq!(
        fs::read_to_string(
            Path::new(task["outputs_dir"].as_str().unwrap()).join("final-message.md")
        )
        .unwrap(),
        "Updated the date handling.\n"
    );

    skill_eval()
        .current_dir(&cwd)
        .args(["dispatch-task", "--dispatch"])
        .arg(&dispatch_path)
        .args(["--task-index", "1"])
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

    skill_eval()
        .current_dir(&cwd)
        .args(["dispatch-task", "--dispatch"])
        .arg(&dispatch_path)
        .args(["--task-index", "0", "--overwrite"])
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
            contains("scripted follow-up turns")
                .and(contains("cool-custom-harness"))
                .and(contains("conversation")),
        );
}
