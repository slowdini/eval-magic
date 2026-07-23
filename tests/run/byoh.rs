//! Bring-your-own-harness end to end: a project-local descriptor file alone
//! carries a complete run — build, dispatch simulation, ingest with the
//! stray-writes audit, and llm_judge grading — plus the `--harness-file`
//! session default and zero-code transcript ingest via a named capability.

use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// A minimal BYOH descriptor: label + exec template, nothing else.
const COOL_DESCRIPTOR: &str = r#"label = "cool-custom-harness"

[dispatch]
exec_template = "cool-cli run --cd <eval-root>{model_arg} <dispatch_prompt_path> > <outputs_dir>/final-message.md"
"#;

/// Write `<cwd>/.eval-magic/harnesses/cool.toml`.
fn write_project_descriptor(cwd: &Path, contents: &str) {
    let dir = cwd.join(".eval-magic").join("harnesses");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("cool.toml"), contents).unwrap();
}

/// Resolve a dispatch.json path (they may be cwd-relative) against `cwd`.
fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// The tasks[] array from the iteration's dispatch.json.
fn dispatch_tasks(cwd: &Path) -> Vec<Value> {
    read_json(&iteration_dir(cwd).join("dispatch.json"))["tasks"]
        .as_array()
        .expect("dispatch.json carries tasks[]")
        .clone()
}

/// Criterion: a descriptor file alone produces a complete llm_judge-graded
/// run with the stray-writes audit — warnings name their fallbacks, and the
/// exec recipe lands in RUNBOOK.md and dispatch-manifest.md.
#[test]
fn descriptor_alone_carries_a_complete_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    write_project_descriptor(&cwd, COOL_DESCRIPTOR);

    // Build the run: every undeclared enhancement warns naming its fallback.
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
            "--agent-model",
            "some-model",
        ])
        .assert()
        .success()
        .stderr(
            contains("declares no skills_dir")
                .and(contains("--no-stage"))
                .and(contains("declares no transcript parser"))
                .and(contains("tokens/duration"))
                .and(contains("unverifiable").not())
                .and(contains("final-message.md"))
                .and(contains("declares no model flag"))
                .and(contains("provenance")),
        );

    // The exec recipe reached both human-facing artifacts.
    let runbook = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    assert!(runbook.contains("cool-cli run"), "{runbook}");
    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(manifest.contains("## Dispatch recipe"), "{manifest}");
    assert!(manifest.contains("cool-cli run"), "{manifest}");

    // Forced --no-stage: nothing was staged, so no task carries a staged slug.
    let tasks = dispatch_tasks(&cwd);
    assert_eq!(tasks.len(), 2, "one eval × two conditions");
    assert!(
        tasks.iter().all(|t| t["staged_skill_slug"].is_null()),
        "no-stage run stages nothing"
    );

    // Simulate the dispatches: recover each final message by hand.
    for task in &tasks {
        let outputs = resolve(&cwd, task["outputs_dir"].as_str().unwrap());
        fs::create_dir_all(&outputs).unwrap();
        fs::write(outputs.join("final-message.md"), "I reviewed the MR.\n").unwrap();
    }

    // Ingest: record-runs from final messages, the stray-writes audit, and
    // grade's llm_judge hand-off all run without any harness code.
    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "cool-custom-harness",
            "--iteration",
            "1",
        ])
        .assert()
        .success()
        .stderr(contains("no transcript parser"));

    assert!(
        iteration_dir(&cwd).join("stray-writes.json").exists(),
        "the stray-writes audit ran"
    );
    let judge_tasks = read_json(&iteration_dir(&cwd).join("judge-tasks.json"));
    assert!(
        judge_tasks["total_tasks"].as_u64().unwrap_or(0) >= 1,
        "llm_judge carries the grading: {judge_tasks}"
    );
    for task in &tasks {
        let record = read_json(&resolve(&cwd, task["run_record_path"].as_str().unwrap()));
        assert_eq!(record["final_message"], "I reviewed the MR.");
    }
}

/// A descriptor without an exec_template warns naming the generic handoff:
/// RUNBOOK.md and dispatch-manifest.md carry guidance, not a copy-pasteable
/// per-task command. (The built-in-harness half of this pin — wired harnesses
/// stay quiet — lives in src/cli/run/util.rs.)
#[test]
fn dispatchless_descriptor_warns_naming_the_generic_handoff() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    write_project_descriptor(&cwd, "label = \"cool-custom-harness\"\n");

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
        .success()
        .stderr(contains("declares no dispatch exec recipe").and(contains("RUNBOOK.md")));
}

/// `--guard` with a harness that exists only in user-supplied descriptors is
/// a hard preflight error: guards are restricted to embedded built-ins (fail-
/// open safety), and a run the user asked to guard must not continue silently
/// unguarded. The message names the detect-stray-writes fallback.
#[test]
fn guard_with_a_user_only_harness_is_rejected_in_preflight() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    write_project_descriptor(&cwd, COOL_DESCRIPTOR);

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
            "--guard",
        ])
        .assert()
        .failure()
        .stderr(
            contains("built-in")
                .and(contains("detect-stray-writes"))
                .and(contains("--guard")),
        );

    assert!(
        !iteration_dir(&cwd).join("dispatch.json").exists(),
        "the run must stop in preflight, before building anything"
    );
}

/// Auto-arm never turns the user-only-descriptor restriction into an error:
/// without an explicit `--guard`, the run proceeds unguarded with a warning
/// naming the fallback.
#[test]
fn auto_guard_stays_off_without_error_on_user_only_harness() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    write_project_descriptor(&cwd, COOL_DESCRIPTOR);

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
        .success()
        .stderr(contains("declares no write guard").and(contains("detect-stray-writes")));

    assert!(
        iteration_dir(&cwd).join("dispatch.json").exists(),
        "the run builds; only explicit --guard is rejected"
    );
}

/// The transcript_check clause of the no-transcript-parser warning is scoped
/// to eval configs that actually use the assertion type.
#[test]
fn transcript_check_warning_fires_only_when_evals_use_it() {
    let evals = r#"{ "skill_name": "mr-review", "evals": [ {
        "id": "e1", "prompt": "review this MR", "expected_output": "a review",
        "assertions": [ { "id": "a1", "type": "transcript_check",
                          "check": "ran tests", "pattern": "cargo test" } ] } ] }"#;
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    write_project_descriptor(&cwd, COOL_DESCRIPTOR);

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
        .success()
        .stderr(
            contains("declares no transcript parser")
                .and(contains("unverifiable"))
                .and(contains("llm_judge"))
                .and(contains("final-message.md")),
        );
}

/// A `--harness-file` descriptor becomes the invocation's default harness
/// when `--harness` is omitted.
#[test]
fn harness_file_label_is_the_default_harness() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let file = tmp.path().join("cool.toml");
    fs::write(&file, COOL_DESCRIPTOR).unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill"])
        .arg("--harness-file")
        .arg(&file)
        .assert()
        .success();

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(
        conditions["harness"], "cool-custom-harness",
        "the one-off descriptor's label is the run's default harness"
    );
}

/// Scope item 4: a user descriptor referencing a built-in transcript parser by
/// name (`codex-items`) gets full transcript ingest with zero code.
#[test]
fn named_capability_gives_zero_code_transcript_ingest() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    write_project_descriptor(
        &cwd,
        r#"label = "cool-custom-harness"

[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "cool-events.jsonl"
parser = "codex-items"

[dispatch]
exec_template = "cool-cli run --cd <eval-root> <dispatch_prompt_path> > <outputs_dir>/cool-events.jsonl"
"#,
    );

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
        .success()
        .stderr(contains("declares no transcript parser").not());

    // Simulate dispatches that emit Codex-compatible JSONL events.
    for task in dispatch_tasks(&cwd) {
        let outputs = resolve(&cwd, task["outputs_dir"].as_str().unwrap());
        fs::create_dir_all(&outputs).unwrap();
        fs::write(outputs.join("final-message.md"), "Done.\n").unwrap();
        fs::write(
            outputs.join("cool-events.jsonl"),
            concat!(
                r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"bash -lc 'cat notes.md'","output":"notes","status":"completed"}}"#,
                "\n",
                r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"Done."}}"#,
                "\n",
            ),
        )
        .unwrap();
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "cool-custom-harness",
            "--iteration",
            "1",
        ])
        .assert()
        .success()
        .stderr(contains("no transcript parser").not());

    // The run records carry parsed tool invocations — full transcript ingest
    // from configuration alone.
    for task in dispatch_tasks(&cwd) {
        let record = read_json(&resolve(&cwd, task["run_record_path"].as_str().unwrap()));
        let invocations = record["tool_invocations"].as_array().unwrap();
        assert_eq!(invocations.len(), 1, "{record}");
        assert_eq!(invocations[0]["name"], "command_execution");
    }
}

/// Criterion: a [transcript.extract] block over an event stream no built-in
/// parser understands still yields full transcript ingest — invocations in
/// run.json plus token/duration backfill into timing.json — with zero code.
#[test]
fn extract_block_gives_zero_code_transcript_ingest_for_a_novel_stream() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    write_project_descriptor(
        &cwd,
        r#"label = "cool-custom-harness"

[tools]
write = ["file_write"]
shell = ["shell"]

[transcript]
events_filename = "cool-events.jsonl"

[transcript.extract.tools]
where = { kind = "tool.call" }
name_field = "tool"
args_omit = ["kind", "tool", "output", "at"]
result_coalesce = ["output"]

[transcript.extract.final_text]
where = { kind = "message" }
field = "text"

[transcript.extract.tokens]
where = { kind = "usage" }
sum = ["in", "out"]

[transcript.extract.duration]
timestamp_spread = "at"

[dispatch]
exec_template = "cool-cli run --cd <eval-root> <dispatch_prompt_path> > <outputs_dir>/cool-events.jsonl"
"#,
    );

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
        .success()
        .stderr(contains("declares no transcript parser").not());

    // Simulate dispatches that emit the harness's own flat stream.
    for task in dispatch_tasks(&cwd) {
        let outputs = resolve(&cwd, task["outputs_dir"].as_str().unwrap());
        fs::create_dir_all(&outputs).unwrap();
        fs::write(outputs.join("final-message.md"), "Done.\n").unwrap();
        fs::write(
            outputs.join("cool-events.jsonl"),
            concat!(
                r#"{"kind":"tool.call","tool":"shell","cmd":"cat notes.md","output":"notes","at":"2026-07-16T09:00:00Z"}"#,
                "\n",
                r#"{"kind":"message","text":"First reply","at":"2026-07-16T09:00:05Z"}"#,
                "\n",
                r#"{"kind":"usage","in":120,"out":30,"at":"2026-07-16T09:00:09Z"}"#,
                "\n",
                r#"{"kind":"message","text":"Done.","at":"2026-07-16T09:00:10Z"}"#,
                "\n",
            ),
        )
        .unwrap();
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "cool-custom-harness",
            "--iteration",
            "1",
        ])
        .assert()
        .success()
        .stderr(contains("no transcript parser").not());

    for task in dispatch_tasks(&cwd) {
        let record = read_json(&resolve(&cwd, task["run_record_path"].as_str().unwrap()));
        let invocations = record["tool_invocations"].as_array().unwrap();
        assert_eq!(invocations.len(), 1, "{record}");
        assert_eq!(invocations[0]["name"], "shell");
        assert_eq!(
            invocations[0]["args"],
            serde_json::json!({"cmd": "cat notes.md"})
        );
        assert_eq!(invocations[0]["result"], "notes");

        let timing = read_json(&resolve(&cwd, task["timing_path"].as_str().unwrap()));
        assert_eq!(timing["total_tokens"], 150, "{timing}");
        assert_eq!(timing["duration_ms"], 10_000, "{timing}");
    }
}
