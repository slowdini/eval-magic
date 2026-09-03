//! Plan-mode evals: the per-eval `plan_mode` declaration, the `run` preflight
//! gates that keep it honest, and how the declaration reaches `dispatch.json`.

use crate::helpers::*;
use predicates::prelude::*;
use predicates::str::contains;

/// One eval that starts in plan mode beside one that does not.
const PLAN_MODE_EVALS: &str = r#"{
  "skill_name": "mr-review",
  "evals": [
    {
      "id": "plan-first",
      "prompt": "Requests to the pricing API are slow. Add caching.",
      "expected_output": "a working cache",
      "plan_mode": true
    },
    { "id": "plain", "prompt": "review this MR", "expected_output": "a review" }
  ]
}"#;

fn run_dry(
    skill_dir: &std::path::Path,
    cwd: &std::path::Path,
    harness: &str,
) -> assert_cmd::assert::Assert {
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
            "--dry-run",
        ])
        .assert()
}

/// `codex exec` has no plan-mode flag, so its descriptor declares no
/// `[plan_mode]` table and a plan-mode eval is refused before any environment
/// is built.
#[test]
fn a_plan_mode_eval_rejects_a_harness_without_native_plan_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), PLAN_MODE_EVALS);

    run_dry(&skill_dir, &cwd, "codex").failure().stderr(
        contains("[plan_mode]")
            .and(contains("plan-first"))
            .and(contains("codex"))
            .and(contains("harness list")),
    );
    assert!(
        !cwd.join(".eval-magic")
            .join("mr-review")
            .join("iteration-1")
            .join("dispatch.json")
            .exists(),
        "the gate fires before a workspace is built"
    );
}

/// OpenCode writes no plan file, so nothing but a responder can tell when the
/// agent has presented its plan: a plan-mode eval there must declare one.
#[test]
fn a_plan_mode_eval_without_a_responder_needs_a_plan_file_signal() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), PLAN_MODE_EVALS);
    run_dry(&skill_dir, &cwd, "opencode").failure().stderr(
        contains("responder")
            .and(contains("plan-first"))
            .and(contains("plan_file"))
            .and(contains("opencode")),
    );

    let with_responder = PLAN_MODE_EVALS.replace(
        "\"plan_mode\": true",
        "\"plan_mode\": true, \"responder\": { \"type\": \"llm\" }",
    );
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &with_responder);
    run_dry(&skill_dir, &cwd, "opencode").success();
}

/// Claude Code declares a plan file, so a responder is optional there; the
/// declaration is announced in the run plan and recorded on every task of the
/// eval — and only there, so other tasks serialize as they always did.
#[test]
fn a_plan_mode_eval_is_announced_and_recorded_per_task() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), PLAN_MODE_EVALS);
    run_dry(&skill_dir, &cwd, "claude-code")
        .success()
        .stdout(contains("plan mode: 1 eval"));

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let tasks = dispatch["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 4, "two evals × two conditions");
    for task in tasks {
        match task["eval_id"].as_str().unwrap() {
            "plan-first" => assert_eq!(task["plan_mode"], serde_json::Value::Bool(true)),
            "plain" => assert!(task.get("plan_mode").is_none(), "{task}"),
            other => panic!("unexpected eval {other}"),
        }
    }
}

// ── The plan phase, driven end to end ─────────────────────────────────────────
//
// Each test below swaps the frozen descriptor's two templates for one POSIX stub
// standing in for Claude Code. The stub is data-driven: a test writes, per round
// label (`initial`, `resume-2`, …), the stream-json events the round emits, the
// permission mode it must have been dispatched with, and optionally the exact
// user text it must have received. Pointed at a responder prompt it copies a
// canned verdict into place instead, the way the responder tests do.

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const APPROVAL: &str = "The plan is approved. Implement it now.";

fn stub_script(dir: &Path) -> PathBuf {
    let script = dir.join("fake-claude.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
# args: outputs prompt_path label flag mode eval_root rounds [session] [prompt]
outputs=$1; prompt_path=$2; label=$3; flag=$4; mode=$5; rounds=$7; session=$8; prompt=$9
events=${EVENTS_FILE:-claude-events.jsonl}
case "$prompt_path" in
  */responder/*)
    round=$(basename "$outputs" | sed 's/^turn-//')
    file="$rounds/verdict-$round.json"
    [ -f "$file" ] || exit 47
    cat "$file" > "$outputs/verdict.json"
    exit 0
    ;;
esac
[ "$flag" = "--permission-mode" ] || exit 41
expected=$(cat "$rounds/$label.mode") || exit 44
[ "$mode" = "$expected" ] || exit 42
if [ "$label" != "initial" ]; then
  [ "$session" = "session-1" ] || exit 43
  if [ -f "$rounds/$label.prompt" ]; then
    [ "$prompt" = "$(cat "$rounds/$label.prompt")" ] || exit 45
  fi
fi
sed "s#@HOME@#$HOME#g" "$rounds/$label.jsonl" > "$outputs/$events"
"#,
    )
    .unwrap();
    script
}

/// Prepare the iteration, then point both frozen templates at the stub.
fn prepare(
    tmp: &Path,
    evals: &str,
    harness: &str,
    extra_run_args: &[&str],
) -> (PathBuf, PathBuf, PathBuf) {
    let (skill_dir, cwd) = setup(tmp, evals);
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
            harness,
            "--no-guard",
        ])
        .args(extra_run_args)
        .assert()
        .success();

    let rounds = tmp.join("rounds");
    fs::create_dir_all(&rounds).unwrap();
    let stub = format!("sh \"{}\"", stub_script(tmp).to_string_lossy());
    let rounds_arg = format!("\"{}\"", rounds.to_string_lossy());
    let dispatch_path = iteration_dir(&cwd).join("dispatch.json");
    let mut dispatch = read_json(&dispatch_path);
    dispatch["harness_descriptor"]["dispatch"]["exec_template"] = json!(format!(
        "{stub} <outputs_dir> <dispatch_prompt_path> initial{{mode_args}} <eval-root> {rounds_arg}"
    ));
    dispatch["harness_descriptor"]["conversation"]["resume_exec_template"] = json!(format!(
        "{stub} <outputs_dir> <dispatch_prompt_path> resume-<round>{{mode_args}} <eval-root> \
         {rounds_arg} {{session_arg}} {{prompt_arg}}"
    ));
    fs::write(
        &dispatch_path,
        format!("{}\n", serde_json::to_string_pretty(&dispatch).unwrap()),
    )
    .unwrap();
    (skill_dir, cwd, rounds)
}

fn dispatch(
    skill_dir: &Path,
    cwd: &Path,
    harness: &str,
    home: &Path,
) -> assert_cmd::assert::Assert {
    skill_eval()
        .current_dir(cwd)
        .env("HOME", home)
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
            "0",
        ])
        .assert()
}

fn init() -> Value {
    json!({"type": "system", "subtype": "init", "session_id": "session-1"})
}

fn tool_use(name: &str, input: Value) -> Vec<Value> {
    vec![
        json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "toolu_1", "name": name, "input": input}
        ]}}),
        json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok", "is_error": false}
        ]}}),
    ]
}

fn result(text: &str) -> Value {
    json!({"type": "result", "subtype": "success", "is_error": false, "result": text,
           "duration_ms": 5, "usage": {"input_tokens": 2, "output_tokens": 3}})
}

fn round(rounds: &Path, label: &str, mode: &str, prompt: Option<&str>, events: &[Value]) {
    let body = events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(rounds.join(format!("{label}.jsonl")), format!("{body}\n")).unwrap();
    fs::write(rounds.join(format!("{label}.mode")), mode).unwrap();
    if let Some(prompt) = prompt {
        fs::write(rounds.join(format!("{label}.prompt")), prompt).unwrap();
    }
}

fn plan_write_events(plan: &str) -> Vec<Value> {
    let mut events = vec![init()];
    events.extend(tool_use(
        "Write",
        json!({"file_path": "@HOME@/.claude/plans/pricing-cache.md", "content": plan}),
    ));
    events.push(result(
        "I have written the plan; here it is for your review.",
    ));
    events
}

fn edit_events(text: &str) -> Vec<Value> {
    let mut events = vec![init()];
    events.extend(tool_use(
        "Edit",
        json!({"file_path": "pricing.py", "old_string": "a - b", "new_string": "a + b"}),
    ));
    events.push(result(text));
    events
}

fn conversation_of(cwd: &Path) -> (Value, Value) {
    let dispatch = read_json(&iteration_dir(cwd).join("dispatch.json"));
    let task = dispatch["tasks"][0].clone();
    let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));
    (task, conversation)
}

const PLAN_ONLY: &str = r#"{
  "skill_name": "mr-review",
  "evals": [{
    "id": "plan-first",
    "prompt": "Requests to the pricing API are slow. Add caching.",
    "expected_output": "a working cache",
    "plan_mode": true
  }]
}"#;

#[test]
fn a_presented_plan_is_approved_and_the_session_continues_in_act_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd, rounds) = prepare(tmp.path(), PLAN_ONLY, "claude-code", &[]);
    let plan = "1. Fix add()\n2. Add a regression test\n";
    round(&rounds, "initial", "plan", None, &plan_write_events(plan));
    round(
        &rounds,
        "resume-2",
        "bypassPermissions",
        Some(APPROVAL),
        &edit_events("Done."),
    );

    dispatch(&skill_dir, &cwd, "claude-code", tmp.path()).success();

    let (task, conversation) = conversation_of(&cwd);
    assert_eq!(conversation["status"], "completed", "{conversation}");
    assert_eq!(conversation["delivered_followups"], 1);
    let events = conversation["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "{conversation}");
    assert_eq!(events[0]["mode"], "plan");
    assert!(events[0].get("origin").is_none());
    assert_eq!(events[1]["round"], 2);
    assert_eq!(events[1]["text"], APPROVAL);
    assert_eq!(events[1]["origin"], json!({"runner": "plan_approval"}));
    assert_eq!(events[1]["mode"], "act");
    assert_eq!(conversation["plan"]["presented_in_round"], 1);
    assert_eq!(conversation["plan"]["approved_in_round"], 2);
    assert_eq!(conversation["plan"]["signal"], "plan_file");

    let outputs = Path::new(task["outputs_dir"].as_str().unwrap());
    let plan_md = outputs.join("plan.md");
    assert_eq!(
        read_str(&plan_md),
        plan,
        "the plan artifact is the plan file's content"
    );
    assert_eq!(
        conversation["plan"]["artifact_path"].as_str().unwrap(),
        wire_path(&plan_md)
    );
    assert!(outputs.join("turn-2").join("claude-events.jsonl").exists());
    assert!(
        !outputs.join("turn-3").exists(),
        "a one-shot eval ends after the act round"
    );
}

#[test]
fn a_plan_phase_without_a_plan_file_and_no_responder_stops_plan_not_presented() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd, rounds) = prepare(tmp.path(), PLAN_ONLY, "claude-code", &[]);
    round(
        &rounds,
        "initial",
        "plan",
        None,
        &[init(), result("Which module owns the pricing client?")],
    );

    dispatch(&skill_dir, &cwd, "claude-code", tmp.path())
        .success()
        .stderr(contains("without presenting a plan"));

    let (task, conversation) = conversation_of(&cwd);
    assert_eq!(conversation["status"], "stopped", "{conversation}");
    assert_eq!(conversation["stop_reason"], "plan_not_presented");
    assert_eq!(conversation["stopped_before_followup"], 1);
    assert!(conversation.get("plan").is_none());
    assert_eq!(conversation["events"].as_array().unwrap().len(), 1);
    let outputs = Path::new(task["outputs_dir"].as_str().unwrap());
    assert!(!outputs.join("turn-2").exists());
    assert!(!outputs.join("plan.md").exists());
}

#[test]
fn the_responder_answers_plan_phase_questions_until_the_plan_is_presented() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = PLAN_ONLY.replace(
        "\"plan_mode\": true",
        "\"plan_mode\": true, \"responder\": { \"type\": \"llm\" }",
    );
    let (skill_dir, cwd, rounds) = prepare(
        tmp.path(),
        &evals,
        "claude-code",
        &["--responder-model", "test-responder-model"],
    );
    round(
        &rounds,
        "initial",
        "plan",
        None,
        &[
            init(),
            result(
                "Which cache backend do you prefer: Redis (needs a service) or an in-memory LRU (recommended)?",
            ),
        ],
    );
    fs::write(
        rounds.join("verdict-1.json"),
        r#"{"verdict": "answer", "reply": "Use the in-memory LRU.", "rationale": "the recommended option"}"#,
    )
    .unwrap();
    round(
        &rounds,
        "resume-2",
        "plan",
        Some("Use the in-memory LRU."),
        &plan_write_events("1. Add an LRU\n"),
    );
    round(
        &rounds,
        "resume-3",
        "bypassPermissions",
        Some(APPROVAL),
        &edit_events("The cache is in place."),
    );
    fs::write(
        rounds.join("verdict-3.json"),
        r#"{"verdict": "done", "rationale": "the agent reported the cache in place"}"#,
    )
    .unwrap();

    dispatch(&skill_dir, &cwd, "claude-code", tmp.path()).success();

    let (_, conversation) = conversation_of(&cwd);
    assert_eq!(conversation["status"], "completed", "{conversation}");
    assert_eq!(conversation["delivered_followups"], 2);
    let events = conversation["events"].as_array().unwrap();
    let modes: Vec<&str> = events.iter().map(|e| e["mode"].as_str().unwrap()).collect();
    assert_eq!(modes, ["plan", "plan", "act"]);
    assert!(events[0].get("origin").is_none());
    assert_eq!(events[1]["origin"]["responder"], "llm");
    assert_eq!(events[1]["origin"]["rationale"], "the recommended option");
    assert_eq!(events[2]["origin"], json!({"runner": "plan_approval"}));
    assert_eq!(conversation["plan"]["presented_in_round"], 2);
    assert_eq!(conversation["plan"]["approved_in_round"], 3);
    assert_eq!(conversation["plan"]["signal"], "plan_file");
    assert_eq!(conversation["responder_outcome"]["ending"], "done");
}

#[test]
fn scripted_turns_follow_the_approval() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = PLAN_ONLY.replace(
        "\"plan_mode\": true",
        "\"plan_mode\": true, \"turns\": [{ \"prompt\": \"Also add a metric.\", \"deliver_when\": \"always\" }]",
    );
    let (skill_dir, cwd, rounds) = prepare(tmp.path(), &evals, "claude-code", &[]);
    round(
        &rounds,
        "initial",
        "plan",
        None,
        &plan_write_events("1. Cache it\n"),
    );
    round(
        &rounds,
        "resume-2",
        "bypassPermissions",
        Some(APPROVAL),
        &edit_events("Cached."),
    );
    round(
        &rounds,
        "resume-3",
        "bypassPermissions",
        Some("Also add a metric."),
        &edit_events("Metric added."),
    );

    dispatch(&skill_dir, &cwd, "claude-code", tmp.path()).success();

    let (_, conversation) = conversation_of(&cwd);
    assert_eq!(conversation["status"], "completed", "{conversation}");
    assert_eq!(conversation["delivered_followups"], 2);
    let events = conversation["events"].as_array().unwrap();
    let texts: Vec<&str> = events.iter().map(|e| e["text"].as_str().unwrap()).collect();
    assert_eq!(
        texts,
        [
            "Requests to the pricing API are slow. Add caching.",
            APPROVAL,
            "Also add a metric."
        ]
    );
    let modes: Vec<&str> = events.iter().map(|e| e["mode"].as_str().unwrap()).collect();
    assert_eq!(modes, ["plan", "act", "act"]);
    assert!(
        events[2].get("origin").is_none(),
        "a scripted turn is authored by the eval"
    );
}

/// A harness that writes no plan file has no signal of its own; the responder's
/// `done` in the planning phase is what approves the plan.
#[test]
fn a_signal_less_harness_approves_on_the_responders_done_verdict() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = PLAN_ONLY.replace(
        "\"plan_mode\": true",
        "\"plan_mode\": true, \"responder\": { \"type\": \"llm\" }",
    );
    let (skill_dir, cwd) = setup(tmp.path(), &evals);
    let descriptor_dir = cwd.join(".eval-magic").join("harnesses");
    fs::create_dir_all(&descriptor_dir).unwrap();
    // Claude-shaped transcripts, so the same stub serves; no plan file declared.
    fs::write(
        descriptor_dir.join("planner.toml"),
        r#"label = "planner"
skills_dir = ".planner/skills"
config_dirs = [".planner"]

[tools]
write = ["Write", "Edit"]
shell = ["Bash"]

[transcript]
events_filename = "planner-events.jsonl"
parser = "claude-stream-json"

[dispatch]
exec_template = "planner{mode_args} <eval-root> <dispatch_prompt_path> > <outputs_dir>/planner-events.jsonl"

[conversation]
resume_exec_template = "planner{mode_args} --resume {session_arg} <eval-root> {prompt_arg} > <outputs_dir>/planner-events.jsonl"

[plan_mode]
plan_args = " --permission-mode plan"
act_args = " --permission-mode act"
"#,
    )
    .unwrap();
    // `prepare` re-runs setup; do the equivalent by hand against the project descriptor.
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
            "planner",
            "--no-guard",
            "--responder-model",
            "m",
        ])
        .assert()
        .success();
    let rounds = tmp.path().join("rounds");
    fs::create_dir_all(&rounds).unwrap();
    let stub = format!(
        "EVENTS_FILE=planner-events.jsonl sh \"{}\"",
        stub_script(tmp.path()).to_string_lossy()
    );
    let rounds_arg = format!("\"{}\"", rounds.to_string_lossy());
    let dispatch_path = iteration_dir(&cwd).join("dispatch.json");
    let mut dispatch_json = read_json(&dispatch_path);
    dispatch_json["harness_descriptor"]["dispatch"]["exec_template"] = json!(format!(
        "{stub} <outputs_dir> <dispatch_prompt_path> initial{{mode_args}} <eval-root> {rounds_arg}"
    ));
    dispatch_json["harness_descriptor"]["conversation"]["resume_exec_template"] = json!(format!(
        "{stub} <outputs_dir> <dispatch_prompt_path> resume-<round>{{mode_args}} <eval-root> \
         {rounds_arg} {{session_arg}} {{prompt_arg}}"
    ));
    fs::write(
        &dispatch_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&dispatch_json).unwrap()
        ),
    )
    .unwrap();

    round(
        &rounds,
        "initial",
        "plan",
        None,
        &[
            init(),
            result("Plan: 1. add an LRU; 2. test it. Shall I proceed?"),
        ],
    );
    fs::write(
        rounds.join("verdict-1.json"),
        r#"{"verdict": "done", "rationale": "the plan is complete"}"#,
    )
    .unwrap();
    round(
        &rounds,
        "resume-2",
        "act",
        Some(APPROVAL),
        &edit_events("Implemented."),
    );
    fs::write(
        rounds.join("verdict-2.json"),
        r#"{"verdict": "done", "rationale": "finished"}"#,
    )
    .unwrap();

    dispatch(&skill_dir, &cwd, "planner", tmp.path()).success();

    let (task, conversation) = conversation_of(&cwd);
    assert_eq!(conversation["status"], "completed", "{conversation}");
    assert_eq!(conversation["plan"]["signal"], "responder");
    assert_eq!(conversation["plan"]["presented_in_round"], 1);
    assert_eq!(conversation["plan"]["approved_in_round"], 2);
    let modes: Vec<&str> = conversation["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["mode"].as_str().unwrap())
        .collect();
    assert_eq!(modes, ["plan", "act"]);
    let plan_md = Path::new(task["outputs_dir"].as_str().unwrap()).join("plan.md");
    assert_eq!(
        read_str(&plan_md),
        "Plan: 1. add an LRU; 2. test it. Shall I proceed?",
        "without a plan file the presenting round's final message is the plan"
    );
}

/// The write guard's boundary is the env, plus the plan-file root the harness
/// declares: Claude Code writes the plan it presents to `~/.claude/plans`, and
/// a guard that denied that write would change how the agent plans.
#[test]
fn the_write_guard_allows_the_declared_plan_file_root() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), PLAN_ONLY);
    skill_eval()
        .current_dir(&cwd)
        .env("HOME", tmp.path())
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--harness",
            "claude-code",
            "--guard",
        ])
        .assert()
        .success();

    let marker = read_json(
        &cli_env_dir(&cwd, "g1", "with_skill").join(".claude/skills/.slow-powers-eval-guard.json"),
    );
    let roots: Vec<String> = marker["allowedRoots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|root| root.as_str().unwrap().to_string())
        .collect();
    assert_eq!(roots.len(), 2, "{roots:?}");
    assert_eq!(
        Path::new(&roots[1]),
        tmp.path().join(".claude").join("plans"),
        "{roots:?}"
    );
}
