//! Bring-your-own-harness end to end: a project-local descriptor file alone
//! carries a complete run — build, dispatch simulation, ingest with the
//! stray-writes audit, and llm_judge grading — plus the `--harness-file`
//! session default and zero-code transcript ingest via named or declarative
//! capabilities.

use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

mod extract;
mod guard_provenance;

/// A runner-ready BYOH descriptor using the named Codex transcript capability.
const COOL_DESCRIPTOR: &str = r#"label = "cool-custom-harness"

[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "cool-events.jsonl"
parser = "codex-items"

[dispatch]
exec_template = "cool-cli run --cd <eval-root>{model_arg} <dispatch_prompt_path> > <outputs_dir>/cool-events.jsonl"
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

fn write_completed_task(cwd: &Path, task: &Value, final_text: &str) {
    let outputs = resolve(cwd, task["outputs_dir"].as_str().unwrap());
    let turn = outputs.join("turn-1");
    fs::create_dir_all(&turn).unwrap();
    fs::write(
        turn.join("cool-events.jsonl"),
        format!(
            "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item_1\",\"type\":\"agent_message\",\"text\":{}}}}}\n",
            serde_json::to_string(final_text).unwrap()
        ),
    )
    .unwrap();
    write_completion(cwd, task);
}

fn write_completion(cwd: &Path, task: &Value) {
    let conversation_path = resolve(cwd, task["conversation_path"].as_str().unwrap());
    fs::write(
        conversation_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "completed",
            "delivered_followups": 0,
            "events": [{
                "type": "user_message",
                "ordinal": 0,
                "round": 1,
                "text": task["user_prompt"]
            }]
        }))
        .unwrap(),
    )
    .unwrap();
}

/// Criterion: a descriptor file alone produces a complete llm_judge-graded
/// run with the stray-writes audit — warnings name their fallbacks, and the
/// exec recipe lands in RUNBOOK.md and dispatch-manifest.md.
#[test]
fn descriptor_alone_carries_a_complete_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    write_project_descriptor(&cwd, COOL_DESCRIPTOR);

    // Build the run: optional undeclared enhancements warn naming their fallback.
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
                .and(contains("declares no model flag"))
                .and(contains("provenance")),
        );

    // The runbook drives through the runner, so it names the command rather
    // than the harness CLI; the manifest still shows what the runner spawns.
    let runbook = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    assert!(runbook.contains("eval-magic dispatch"), "{runbook}");
    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(manifest.contains("## Dispatch"), "{manifest}");
    assert!(manifest.contains("cool-cli run"), "{manifest}");

    // Forced --no-stage: nothing was staged, so no task carries a staged slug.
    let tasks = dispatch_tasks(&cwd);
    assert_eq!(tasks.len(), 2, "one eval × two conditions");
    assert!(
        tasks.iter().all(|t| t["staged_skill_slug"].is_null()),
        "no-stage run stages nothing"
    );

    // Simulate runner-owned completion metadata and per-round transcripts.
    for task in &tasks {
        write_completed_task(&cwd, task, "I reviewed the MR.");
    }

    // Ingest: record-runs from transcripts, the stray-writes audit, and
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
        .success();

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

/// A descriptor without an exec_template is rejected before a workspace is
/// built because the runner has no command to spawn.
#[test]
fn dispatchless_descriptor_is_rejected_before_build() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    write_project_descriptor(
        &cwd,
        &COOL_DESCRIPTOR.replace(
            "\n[dispatch]\nexec_template = \"cool-cli run --cd <eval-root>{model_arg} <dispatch_prompt_path> > <outputs_dir>/cool-events.jsonl\"\n",
            "\n",
        ),
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
        .failure()
        .stderr(
            contains("declares no dispatch exec template")
                .and(contains("runner-ready"))
                .and(contains("eval-magic docs byoh")),
        );

    assert!(!iteration_dir(&cwd).join("dispatch.json").exists());
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

/// A transcript parser is a runner-readiness requirement regardless of which
/// assertion types the eval declares.
#[test]
fn transcriptless_descriptor_is_rejected_before_build() {
    let evals = r#"{ "skill_name": "mr-review", "evals": [ {
        "id": "e1", "prompt": "review this MR", "expected_output": "a review",
        "assertions": [ { "id": "a1", "type": "transcript_check",
                          "check": "ran tests", "pattern": "cargo test" } ] } ] }"#;
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    write_project_descriptor(
        &cwd,
        r#"label = "cool-custom-harness"

[dispatch]
exec_template = "cool-cli run --cd <eval-root> <dispatch_prompt_path>"
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
        .failure()
        .stderr(
            contains("declares no transcript parser")
                .and(contains("runner-ready"))
                .and(contains("eval-magic docs byoh")),
        );

    assert!(!iteration_dir(&cwd).join("dispatch.json").exists());
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
        let turn = outputs.join("turn-1");
        fs::create_dir_all(&turn).unwrap();
        fs::write(
            turn.join("cool-events.jsonl"),
            concat!(
                r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"bash -lc 'cat notes.md'","aggregated_output":"notes","status":"completed"}}"#,
                "\n",
                r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"Done."}}"#,
                "\n",
            ),
        )
        .unwrap();
        write_completion(&cwd, &task);
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

/// Issue #294: a run prepared with `--harness-file` must re-emit the flag in
/// every generated follow-up command — the printed `Next:` block and every
/// `eval-magic …` line in RUNBOOK.md — or follow-ups silently resolve a
/// different descriptor than the run was prepared with, while the iteration's
/// artifacts keep the prep-time declarations.
#[test]
fn harness_file_is_reemitted_in_every_generated_command() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let file = tmp.path().join("cool.toml");
    fs::write(&file, COOL_DESCRIPTOR).unwrap();

    let assert = skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill"])
        .arg("--harness-file")
        .arg(&file)
        .assert()
        .success();

    let flag = format!("--harness-file {}", wire_path(&resolved(&file)));
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains(&flag),
        "the printed Next: block re-emits the flag: {stdout}"
    );

    let runbook = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    let commands: Vec<&str> = runbook
        .lines()
        .filter(|line| line.starts_with("eval-magic "))
        .collect();
    assert!(!commands.is_empty(), "the runbook carries commands");
    for command in commands {
        assert!(
            command.contains(&flag),
            "every runbook command re-emits --harness-file: {command}"
        );
    }

    // Prep-time provenance for the drift backstop: the descriptor the run was
    // prepared with is recorded next to the conditions it produced.
    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(conditions["harness_file"], wire_path(&resolved(&file)));
    let digest = conditions["harness_descriptor_digest"]
        .as_str()
        .expect("conditions record the resolved descriptor digest");
    assert_eq!(digest.len(), 16, "FNV-1a hex digest: {digest}");
}

/// Issue #294 backstop: a follow-up stage that resolves a descriptor different
/// from the prep-time one warns loudly instead of silently switching. The
/// overlay keeps the built-in label so the flag-less follow-up *resolves*
/// (against the un-overlaid built-in) instead of failing on an unknown label.
#[test]
fn dispatch_and_ingest_warn_when_the_resolved_descriptor_drifted_from_prep() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // Overlay the claude-code label, retuning dispatch onto a missing binary so
    // nothing real is ever spawned; every other field merges from the built-in,
    // including its [plan_mode] table, so the template keeps the {mode_args}
    // slot that table fills.
    let file = tmp.path().join("iso.toml");
    fs::write(
        &file,
        r#"label = "claude-code"

[dispatch]
exec_template = "definitely-missing-cli{mode_args} <dispatch_prompt_path>"
"#,
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill"])
        .arg("--harness-file")
        .arg(&file)
        .assert()
        .success();

    // The flag-less follow-up resolves the un-overlaid built-in descriptor:
    // the digest no longer matches the prep-time one.
    skill_eval()
        .current_dir(&cwd)
        .args(["dispatch", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .stderr(
            contains("harness descriptor drift")
                .and(contains("--harness-file"))
                .and(contains(wire_path(&file))),
        );
    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .stderr(contains("harness descriptor drift"));

    // Re-emitting the flag resolves the same descriptor the run was prepared
    // with: digests match, no drift warning. (Dispatch still fails: the
    // overlaid exec template names a missing binary.)
    skill_eval()
        .current_dir(&cwd)
        .arg("--harness-file")
        .arg(&file)
        .args(["dispatch", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .stderr(contains("harness descriptor drift").not());
}

/// #308: a BYOH descriptor opts into portable tool patterns through its
/// `[tools]` vocabulary alone. `zap_exec` is a name no other descriptor knows,
/// yet declaring it in the `shell` role is enough for a frozen `Bash|Read`
/// assertion — authored against a different harness — to grade here.
#[test]
fn a_user_descriptor_opts_into_portable_tool_patterns_through_tools_alone() {
    let evals = r#"{ "skill_name": "mr-review", "evals": [ {
        "id": "e1", "prompt": "review this MR", "expected_output": "a review",
        "skill_should_trigger": false,
        "assertions": [ { "id": "ran-a-command", "type": "transcript_check",
                          "check": "tool_invocation_matches", "pattern": "Bash|Read" } ] } ] }"#;
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    write_project_descriptor(
        &cwd,
        r#"label = "cool-custom-harness"

[tools]
write = ["file_change"]
shell = ["zap_exec"]

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
        .success();

    for task in dispatch_tasks(&cwd) {
        let outputs = resolve(&cwd, task["outputs_dir"].as_str().unwrap());
        let turn = outputs.join("turn-1");
        fs::create_dir_all(&turn).unwrap();
        fs::write(
            turn.join("cool-events.jsonl"),
            concat!(
                r#"{"type":"item.completed","item":{"id":"item_1","type":"zap_exec","command":"cargo test","aggregated_output":"ok","status":"completed"}}"#,
                "\n",
                r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"Done."}}"#,
                "\n",
            ),
        )
        .unwrap();
        write_completion(&cwd, &task);
    }

    for stage in ["ingest", "grade"] {
        let mut cmd = skill_eval();
        cmd.current_dir(&cwd)
            .args([stage, "--skill-dir"])
            .arg(&skill_dir)
            .args([
                "--skill",
                "mr-review",
                "--harness",
                "cool-custom-harness",
                "--iteration",
                "1",
            ]);
        if stage == "grade" {
            cmd.arg("--finalize");
        }
        cmd.assert().success();
    }

    for task in dispatch_tasks(&cwd) {
        let run_record = resolve(&cwd, task["run_record_path"].as_str().unwrap());
        let grading = read_json(&run_record.with_file_name("grading.json"));
        let result = &grading["assertion_results"][0];
        assert_eq!(result["id"], "ran-a-command", "{grading}");
        assert_eq!(
            result["passed"], true,
            "the descriptor's shell role must carry the portable alias: {grading}"
        );
        let evidence = result["evidence"].as_str().unwrap();
        assert!(
            evidence.contains("via shell alias 'Bash'") && evidence.contains("zap_exec"),
            "evidence names the alias and the native event: {evidence}"
        );
    }
}
