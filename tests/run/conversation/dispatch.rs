//! The batch behaviors `eval-magic dispatch` owns: running a whole plan, what a
//! rerun may skip, how a failure and a timeout are recorded, and concurrency.
//!
//! The single-task driver those batches call lives beside this module, in
//! [`super`].

use super::{
    ONE_SHOT_EVALS, dispatch_one, one_shot_stub, prepare_one_shot_run, stub_exec_template,
};
use crate::helpers::*;
use predicates::str::contains;
use std::fs;
use std::path::Path;

/// `dispatch` drives every task in the plan from one command, which is the
/// whole point of the ticket: no operator pastes a per-task recipe any more.
#[test]
fn dispatch_drives_every_task_in_the_plan() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), ONE_SHOT_EVALS);
    prepare_one_shot_run(&skill_dir, &cwd, "codex");
    stub_exec_template(
        &cwd,
        &one_shot_stub(tmp.path(), "Updated the date handling."),
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
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let tasks = dispatch["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2, "both conditions dispatch");
    for task in tasks {
        let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));
        assert_eq!(conversation["status"], "completed", "{conversation}");
        assert_eq!(conversation["delivered_followups"], 0);
    }
}

/// Rerunning a dispatch must not redo finished work: the completion artifact is
/// the marker, so a second run skips what completed and retries only what did
/// not. Proven by a counter the stub appends to once per invocation.
#[test]
fn rerunning_dispatch_skips_completed_tasks_and_retries_the_rest() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), ONE_SHOT_EVALS);
    prepare_one_shot_run(&skill_dir, &cwd, "codex");
    let counter = tmp.path().join("dispatch-count.log");
    stub_exec_template(
        &cwd,
        &counting_stub(tmp.path(), &counter, "Updated the date handling."),
    );

    // Dispatch one task, leaving the other without a completion artifact.
    dispatch_one(&skill_dir, &cwd, "codex", 0, false)
        .assert()
        .success();
    assert_eq!(dispatch_count(&counter), 1);

    // The whole batch: the finished task is skipped, the other one runs.
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
        .stdout(contains(
            "1 completed, 0 stopped, 0 timed out, 0 failed, 1 skipped",
        ));
    assert_eq!(
        dispatch_count(&counter),
        2,
        "the completed task must not be dispatched twice"
    );
}

/// A failing task is campaign data, not a reason to abandon the batch: the rest
/// still runs, the failure is named, and the command exits nonzero so a script
/// notices.
#[test]
fn a_failing_task_is_recorded_and_the_rest_of_the_batch_still_runs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), ONE_SHOT_EVALS);
    prepare_one_shot_run(&skill_dir, &cwd, "codex");

    // Fails for the first condition's env, succeeds for the second.
    let script = tmp.path().join("fail-one.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
outputs=$1
eval_root=$2
case "$eval_root" in
  *-with_skill) exit 9 ;;
esac
printf '%s
' '{"type":"thread.started","thread_id":"session-1"}' > "$outputs/codex-events.jsonl"
printf '%s
' '{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"Done."}}' >> "$outputs/codex-events.jsonl"
printf '%s
' '{"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}' >> "$outputs/codex-events.jsonl"
"#,
    )
    .unwrap();
    stub_exec_template(
        &cwd,
        &format!(
            "sh \"{}\" <outputs_dir> <eval-root>",
            script.to_string_lossy()
        ),
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
        .failure()
        .stdout(contains(
            "1 completed, 0 stopped, 0 timed out, 1 failed, 0 skipped",
        ))
        .stderr(contains("one-shot:with_skill"));

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let tasks = dispatch["tasks"].as_array().unwrap();
    let failed = Path::new(tasks[0]["conversation_path"].as_str().unwrap());
    let succeeded = Path::new(tasks[1]["conversation_path"].as_str().unwrap());
    assert!(
        !failed.exists(),
        "a failed task writes no completion artifact, so a rerun retries it"
    );
    assert!(succeeded.is_file(), "the healthy task still completed");
}

/// A hung dispatch must not hang the campaign: it is killed at the deadline,
/// recorded as timed out, and every other task still finishes. Without this,
/// `execute_round` ran to completion however long that took.
#[test]
fn a_task_that_outruns_the_timeout_is_recorded_and_the_batch_finishes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), ONE_SHOT_EVALS);
    prepare_one_shot_run(&skill_dir, &cwd, "codex");

    // `with_skill` hangs well past the deadline; the other arm answers at once.
    let script = tmp.path().join("hang-one.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
outputs=$1
eval_root=$2
exe=$3
case "$eval_root" in
  *-with_skill) "$exe" __fixture --sleep-ms 5000 ;;
esac
printf '%s
' '{"type":"thread.started","thread_id":"session-1"}' > "$outputs/codex-events.jsonl"
printf '%s
' '{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"Done."}}' >> "$outputs/codex-events.jsonl"
printf '%s
' '{"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}' >> "$outputs/codex-events.jsonl"
"#,
    )
    .unwrap();
    stub_exec_template(
        &cwd,
        &format!(
            "sh \"{}\" <outputs_dir> <eval-root> \"{}\"",
            script.to_string_lossy(),
            env!("CARGO_BIN_EXE_eval-magic")
        ),
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
            "--timeout",
            "1",
        ])
        .assert()
        .failure()
        .stdout(contains(
            "1 completed, 0 stopped, 1 timed out, 0 failed, 0 skipped",
        ));

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let tasks = dispatch["tasks"].as_array().unwrap();
    let hung = read_json(Path::new(tasks[0]["conversation_path"].as_str().unwrap()));
    assert_eq!(hung["status"], "timed_out", "{hung}");
    assert_eq!(hung["timed_out_in_round"], 1);
    assert!(
        hung["duration_ms"].as_u64().unwrap() >= 900,
        "the timeout record retains time spent in the killed harness: {hung}"
    );
    let healthy = read_json(Path::new(tasks[1]["conversation_path"].as_str().unwrap()));
    assert_eq!(healthy["status"], "completed", "{healthy}");
}

/// `--jobs` runs tasks concurrently. Each task is a private environment, so
/// they are independent; four one-second dispatches must therefore finish in
/// well under the four seconds they would take in sequence.
#[test]
fn jobs_runs_tasks_concurrently() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{
      "skill_name": "mr-review",
      "evals": [
        {"id": "a", "prompt": "Fix the date.", "expected_output": "fixed"},
        {"id": "b", "prompt": "Fix the time.", "expected_output": "fixed"}
      ]
    }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    prepare_one_shot_run(&skill_dir, &cwd, "codex");

    let script = tmp.path().join("slow-stub.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
outputs=$1
exe=$2
"$exe" __fixture --sleep-ms 1000
printf '%s
' '{"type":"thread.started","thread_id":"session-1"}' > "$outputs/codex-events.jsonl"
printf '%s
' '{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"Done."}}' >> "$outputs/codex-events.jsonl"
printf '%s
' '{"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}' >> "$outputs/codex-events.jsonl"
"#,
    )
    .unwrap();
    stub_exec_template(
        &cwd,
        &format!(
            "sh \"{}\" <outputs_dir> \"{}\"",
            script.to_string_lossy(),
            env!("CARGO_BIN_EXE_eval-magic")
        ),
    );

    let started = std::time::Instant::now();
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
            "--jobs",
            "4",
        ])
        .assert()
        .success()
        .stdout(contains("4 completed"));
    let elapsed = started.elapsed();

    // Four sequential one-second dispatches take at least four seconds. The
    // ceiling is deliberately loose — this asserts concurrency happened, not
    // how fast a loaded CI runner schedules four processes.
    assert!(
        elapsed < std::time::Duration::from_millis(2500),
        "four concurrent 1s dispatches took {elapsed:?}, which is serial"
    );
}

/// Mode B dispatches through the same command Mode A does. Its conditions are
/// two skill revisions rather than skill-versus-none, and nothing about how a
/// task is driven may depend on which mode produced it.
#[test]
fn revision_mode_dispatches_through_the_same_command() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), ONE_SHOT_EVALS);

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
    stub_exec_template(
        &cwd,
        &one_shot_stub(tmp.path(), "Updated the date handling."),
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
    let tasks = dispatch["tasks"].as_array().unwrap();
    let mut conditions: Vec<&str> = tasks
        .iter()
        .map(|task| task["condition"].as_str().unwrap())
        .collect();
    conditions.sort_unstable();
    assert_eq!(
        conditions,
        ["new_skill", "old_skill"],
        "revision mode compares two skill revisions"
    );
    for task in tasks {
        let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));
        assert_eq!(conversation["status"], "completed", "{conversation}");
    }
}

/// A stub that records each invocation, so a test can prove how many dispatches
/// actually happened.
fn counting_stub(dir: &Path, counter: &Path, message: &str) -> String {
    let script = dir.join("counting-stub.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
outputs=$1
counter=$2
message=$3
printf 'x
' >> "$counter"
printf '%s
' '{"type":"thread.started","thread_id":"session-1"}' > "$outputs/codex-events.jsonl"
printf '%s
' "{\"type\":\"item.completed\",\"item\":{\"id\":\"m1\",\"type\":\"agent_message\",\"text\":\"$message\"}}" >> "$outputs/codex-events.jsonl"
printf '%s
' '{"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}' >> "$outputs/codex-events.jsonl"
"#,
    )
    .unwrap();
    format!(
        "sh \"{}\" <outputs_dir> \"{}\" \"{message}\"",
        script.to_string_lossy(),
        counter.to_string_lossy()
    )
}

fn dispatch_count(counter: &Path) -> usize {
    fs::read_to_string(counter)
        .map(|body| body.lines().count())
        .unwrap_or(0)
}
