//! Cline permission-denial capture and record-runs integration: a refused
//! call lands as a `content_end` tool event whose output is
//! `{"error": "<reason>"}` (3.0.53 evidence, see docs/cline-notes.md), and the
//! `cline-json` parser reads the runtime's policy wordings plus the shared
//! `eval guard: ` prefix out of it.

use crate::helpers::*;
use predicates::str::contains;
use std::fs;
use std::path::Path;

fn resolve(cwd: &Path, path: &str) -> std::path::PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Write a Cline `--json` events file with one refused `editor` call
/// (`content_end` output `{"error"}` carrying the runtime's policy-disabled
/// wording) plus the terminal `run_result`.
fn write_cline_events_with_denial(outputs_dir: &Path) {
    let body = concat!(
        r#"{"ts":"2026-08-13T03:00:00.000Z","type":"agent_event","event":{"type":"content_start","contentType":"tool","toolCallId":"editor_0","toolName":"editor","input":{"path":"/etc/passwd","new_text":"x"}}}"#,
        "\n",
        r#"{"ts":"2026-08-13T03:00:01.000Z","type":"agent_event","event":{"type":"content_end","contentType":"tool","toolCallId":"editor_0","toolName":"editor","output":{"error":"Tool \"editor\" is disabled by policy"}}}"#,
        "\n",
        r#"{"ts":"2026-08-13T03:00:02.000Z","type":"run_result","finishReason":"completed","iterations":1,"usage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":40,"cacheWriteTokens":0,"totalCost":0.001},"aggregateUsage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":40,"cacheWriteTokens":0,"totalCost":0.001},"durationMs":2000,"text":"Blocked."}"#,
        "\n"
    );
    fs::write(outputs_dir.join("cline-events.jsonl"), body).unwrap();
}

/// Write a denial-less Cline events file (one assistant text + `run_result`).
fn write_cline_events_without_denials(outputs_dir: &Path, final_text: &str) {
    let body = format!(
        concat!(
            r#"{{"ts":"2026-08-13T03:00:00.000Z","type":"agent_event","event":{{"type":"content_end","contentType":"text","text":{text}}}}}"#,
            "\n",
            r#"{{"ts":"2026-08-13T03:00:01.000Z","type":"run_result","finishReason":"completed","iterations":1,"usage":{{"inputTokens":10,"outputTokens":2,"cacheReadTokens":0,"cacheWriteTokens":0,"totalCost":0.0}},"aggregateUsage":{{"inputTokens":10,"outputTokens":2,"cacheReadTokens":0,"cacheWriteTokens":0,"totalCost":0.0}},"durationMs":1000,"text":{text}}}"#,
            "\n"
        ),
        text = serde_json::to_string(final_text).unwrap()
    );
    fs::write(outputs_dir.join("cline-events.jsonl"), body).unwrap();
}

#[test]
fn cline_record_runs_reports_permission_denials_from_the_event_stream() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "cline"])
        .assert()
        .success();

    let tasks = read_json(&iteration_dir(&cwd).join("dispatch.json"))["tasks"]
        .as_array()
        .expect("dispatch.json carries tasks[]")
        .clone();
    assert_eq!(tasks.len(), 2, "{tasks:?}");
    for task in &tasks {
        let outputs = resolve(&cwd, task["outputs_dir"].as_str().unwrap());
        fs::create_dir_all(&outputs).unwrap();
        fs::write(outputs.join("final-message.md"), "Reviewed.\n").unwrap();
        if task["condition"] == "with_skill" {
            write_cline_events_with_denial(&outputs);
        } else {
            write_cline_events_without_denials(&outputs, "Reviewed.");
        }
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["record-runs", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--workspace-dir"])
        .arg(cwd.join(".eval-magic"))
        .args(["--harness", "cline", "--iteration", "1"])
        .assert()
        .success()
        .stderr(contains("permission-denied"))
        .stderr(contains("permission-denials.json"));

    let report = read_json(&iteration_dir(&cwd).join("permission-denials.json"));
    assert_eq!(report["iteration"], 1, "{report}");
    assert_eq!(report["total_denials"], 1, "{report}");
    let reported = report["tasks"].as_array().unwrap();
    assert_eq!(reported.len(), 1, "{report}");
    assert_eq!(reported[0]["condition"], "with_skill");
    assert_eq!(reported[0]["guard_attributed_count"], 0);
    assert_eq!(reported[0]["denials"][0]["tool"], "editor");
    assert_eq!(
        reported[0]["denials"][0]["reason"],
        "Tool \"editor\" is disabled by policy"
    );
    assert_eq!(
        reported[0]["denials"][0]["input_keys"],
        serde_json::json!(["new_text", "path"])
    );

    for task in &tasks {
        assert!(
            resolve(&cwd, task["run_record_path"].as_str().unwrap()).exists(),
            "{task:?}"
        );
    }
}
