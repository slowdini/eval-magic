//! Runner-driven judge dispatch: `eval-magic dispatch --judges`.

use crate::helpers::*;
use predicates::str::contains;
use std::fs;
use std::path::Path;

/// Two `llm_judge` assertions, so both arms emit judge tasks and one condition
/// carries more than one — which is what makes a shared capture path collide.
const JUDGED_EVALS: &str = r#"{
  "skill_name": "mr-review",
  "evals": [{
    "id": "reviewed",
    "prompt": "Review this MR.",
    "expected_output": "a clear review",
    "assertions": [
      {"id": "clear", "type": "llm_judge", "rubric": "Was the review clear?"},
      {"id": "concise", "type": "llm_judge", "rubric": "Was the review concise?"}
    ]
  }]
}"#;

const SAMPLED_JUDGED_EVALS: &str = r#"{
  "skill_name": "mr-review",
  "evals": [{
    "id": "reviewed",
    "prompt": "Review this MR.",
    "expected_output": "a clear review",
    "assertions": [
      {"id": "clear", "type": "llm_judge", "rubric": "Was the review clear?", "samples": 2}
    ]
  }]
}"#;

/// The runner dispatches judge tasks the same way it dispatches eval tasks: it
/// skips verdicts that already exist, runs the ones that do not, and reports
/// how many are present. Before this, an operator pasted a `jq`/`xargs`
/// pipeline out of `RUNBOOK.md` to do it.
#[test]
fn dispatch_judges_runs_missing_verdicts_and_skips_present_ones() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), JUDGED_EVALS);
    prepare_and_dispatch(tmp.path(), &skill_dir, &cwd);

    let judge_tasks = read_json(&iteration_dir(&cwd).join("judge-tasks.json"));
    let tasks = judge_tasks["tasks"].as_array().unwrap().clone();
    assert!(tasks.len() >= 2, "both conditions judge: {judge_tasks}");

    // Pre-answer the first task; the runner must leave it alone.
    let answered = Path::new(tasks[0]["response_path"].as_str().unwrap());
    fs::create_dir_all(answered.parent().unwrap()).unwrap();
    fs::write(
        answered,
        r#"{"passed":true,"evidence":"pre-existing","confidence":0.9}"#,
    )
    .unwrap();

    stub_judge_template(&cwd, &judge_stub(tmp.path()));

    skill_eval()
        .current_dir(&cwd)
        .args(["dispatch", "--judges", "--skill-dir"])
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
        .stdout(contains(format!(
            "{}/{} verdicts present",
            tasks.len(),
            tasks.len()
        )));

    assert_eq!(
        fs::read_to_string(answered).unwrap(),
        r#"{"passed":true,"evidence":"pre-existing","confidence":0.9}"#,
        "an existing verdict is never redispatched"
    );
    for task in &tasks[1..] {
        let response = read_json(Path::new(task["response_path"].as_str().unwrap()));
        assert_eq!(response["evidence"], "stub verdict", "{response}");
    }
}

/// Every judge task captures its transcript in its own directory. Several
/// assertions share one `judge-responses/` directory, so binding the capture to
/// that directory would have them overwrite each other's events file.
#[test]
fn each_judge_task_captures_its_transcript_separately() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), JUDGED_EVALS);
    prepare_and_dispatch(tmp.path(), &skill_dir, &cwd);
    stub_judge_template(&cwd, &judge_stub(tmp.path()));

    skill_eval()
        .current_dir(&cwd)
        .args(["dispatch", "--judges", "--skill-dir"])
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

    let judge_tasks = read_json(&iteration_dir(&cwd).join("judge-tasks.json"));
    let captures: Vec<_> = judge_tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| {
            let response = Path::new(task["response_path"].as_str().unwrap());
            response.with_extension("").join("codex-events.jsonl")
        })
        .collect();
    for capture in &captures {
        assert!(capture.is_file(), "missing judge transcript {capture:?}");
    }
    let distinct: std::collections::BTreeSet<_> = captures.iter().collect();
    assert_eq!(
        distinct.len(),
        captures.len(),
        "each judge task needs its own capture path"
    );
}

/// A missing verdict is reported and exits nonzero, so a script can tell a
/// finished judge batch from one that still needs a rerun.
#[test]
fn dispatch_judges_exits_nonzero_while_a_verdict_is_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), JUDGED_EVALS);
    prepare_and_dispatch(tmp.path(), &skill_dir, &cwd);

    // A judge that answers nothing: the batch runs, but no verdict lands.
    let script = tmp.path().join("silent-judge.sh");
    fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    stub_judge_template(&cwd, &format!("sh \"{}\"", script.to_string_lossy()));

    skill_eval()
        .current_dir(&cwd)
        .args(["dispatch", "--judges", "--skill-dir"])
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
        .stdout(contains("0/"))
        .stderr(contains("verdict"));
}

/// Sample coordinates belong in dispatch failures so an operator can rerun
/// the exact missing judge without confusing it with another independent vote.
#[test]
fn sampled_judge_failures_name_the_sample() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), SAMPLED_JUDGED_EVALS);
    prepare_and_dispatch(tmp.path(), &skill_dir, &cwd);

    let script = tmp.path().join("fail-second-sample.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
outputs=$1
case "$outputs" in
  *__sample-2) exit 7 ;;
  *) printf '%s\n' '{"passed":true,"evidence":"stub verdict","confidence":0.8}' > "${outputs}.json" ;;
esac
"#,
    )
    .unwrap();
    stub_judge_template(
        &cwd,
        &format!("sh \"{}\" <outputs_dir>", script.to_string_lossy()),
    );

    skill_eval()
        .current_dir(&cwd)
        .args(["dispatch", "--judges", "--skill-dir"])
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
        .stderr(contains("reviewed:with_skill:clear:sample-2-of-2"))
        .stderr(contains("reviewed:without_skill:clear:sample-2-of-2"));
}

/// Prepare an iteration, dispatch its eval tasks through a stub, and ingest, so
/// `judge-tasks.json` exists to dispatch judges from.
fn prepare_and_dispatch(tmp: &Path, skill_dir: &Path, cwd: &Path) {
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

    let script = tmp.join("eval-stub.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
outputs=$1
printf '%s\n' '{"type":"thread.started","thread_id":"session-1"}' > "$outputs/codex-events.jsonl"
printf '%s\n' '{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"I reviewed the MR."}}' >> "$outputs/codex-events.jsonl"
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}' >> "$outputs/codex-events.jsonl"
"#,
    )
    .unwrap();
    set_descriptor_template(
        cwd,
        "exec_template",
        &format!("sh \"{}\" <outputs_dir>", script.to_string_lossy()),
    );

    skill_eval()
        .current_dir(cwd)
        .args(["dispatch", "--skill-dir"])
        .arg(skill_dir)
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

    // Judge tasks are emitted here. The exit status is deliberately discarded:
    // ingest exits nonzero while verdicts are outstanding, which is exactly the
    // state this fixture wants to hand to the judge dispatcher.
    let _ = skill_eval()
        .current_dir(cwd)
        .args(["ingest", "--skill-dir"])
        .arg(skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--iteration",
            "1",
            "--harness",
            "codex",
        ])
        .assert();
}

/// A judge stub: derives the verdict path from its capture directory the way a
/// real judge reads it out of its prompt, and writes a verdict there.
fn judge_stub(dir: &Path) -> String {
    let script = dir.join("judge-stub.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
outputs=$1
printf '%s\n' '{"type":"thread.started","thread_id":"judge-1"}' > "$outputs/codex-events.jsonl"
printf '%s\n' '{"passed":true,"evidence":"stub verdict","confidence":0.8}' > "${outputs}.json"
"#,
    )
    .unwrap();
    format!("sh \"{}\" <outputs_dir>", script.to_string_lossy())
}

fn stub_judge_template(cwd: &Path, template: &str) {
    set_descriptor_template(cwd, "exec_template", template);
}

/// Swap one template in the frozen descriptor `dispatch.json` carries.
fn set_descriptor_template(cwd: &Path, field: &str, template: &str) {
    let dispatch_path = iteration_dir(cwd).join("dispatch.json");
    let mut dispatch = read_json(&dispatch_path);
    dispatch["harness_descriptor"]["dispatch"][field] = serde_json::json!(template);
    fs::write(
        &dispatch_path,
        format!("{}\n", serde_json::to_string_pretty(&dispatch).unwrap()),
    )
    .unwrap();
}

/// A judge runs from the iteration directory, outside every guarded task env,
/// so it must not inherit the eval dispatch's hook-trust bypass. The stub
/// records the guard fragment it was handed.
#[test]
fn a_judge_dispatch_carries_no_guard_arguments() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), JUDGED_EVALS);
    prepare_and_dispatch(tmp.path(), &skill_dir, &cwd);

    let seen = tmp.path().join("guard-args.log");
    let script = tmp.path().join("guard-probe.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
outputs=$1
guard=$2
printf '[%s]\n' "$guard" >> "$3"
printf '%s\n' '{"passed":true,"evidence":"stub verdict","confidence":0.8}' > "${outputs}.json"
"#,
    )
    .unwrap();
    // `{guard_args}` renders as the descriptor's fragment when the guard is on
    // and as the empty string when it is off.
    set_descriptor_template(
        &cwd,
        "exec_template",
        &format!(
            "sh \"{}\" <outputs_dir> \"{{guard_args}}\" \"{}\"",
            script.to_string_lossy(),
            seen.to_string_lossy()
        ),
    );

    skill_eval()
        .current_dir(&cwd)
        .args(["dispatch", "--judges", "--skill-dir"])
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

    let recorded = fs::read_to_string(&seen).unwrap();
    assert!(!recorded.trim().is_empty(), "the judge stub ran");
    for line in recorded.lines() {
        assert_eq!(
            line, "[]",
            "a judge must get no guard arguments: {recorded}"
        );
    }
}
