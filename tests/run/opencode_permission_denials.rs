//! OpenCode permission-denial capture and record-runs integration.

use crate::helpers::*;
use predicates::str::contains;
use std::fs;
use std::path::Path;

/// The fixed prefix OpenCode's `PermissionDeniedError` emits before its
/// ruleset JSON — matches the parser constant so this test breaks if the
/// marker drifts.
const OPENCODE_DENY_RULE_PREFIX: &str =
    "The user has specified a rule which prevents you from using this specific tool call.";

fn resolve(cwd: &Path, path: &str) -> std::path::PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Write an OpenCode `--format json` events file with one refused `pwd` call
/// (`state.status:"error"` carrying the deny-rule prefix) plus a final `text`.
fn write_opencode_events_with_denial(outputs_dir: &Path) {
    let error = serde_json::to_string(&format!(
        "{OPENCODE_DENY_RULE_PREFIX} Here are some of the relevant rules []"
    ))
    .unwrap();
    let body = format!(
        concat!(
            r#"{{"type":"text","timestamp":1000,"sessionID":"ses_1","part":{{"id":"p1","type":"text","text":"Verified by reasoning."}}}}"#,
            "\n",
            r#"{{"type":"step_finish","timestamp":2000,"sessionID":"ses_1","part":{{"id":"p2","type":"step-finish","reason":"stop","tokens":{{"input":1,"output":1,"reasoning":0,"cache":{{"read":0,"write":0}}}}}}}}"#,
            "\n",
            r#"{{"type":"tool_use","timestamp":1500,"sessionID":"ses_1","part":{{"id":"pt_1","sessionID":"ses_1","messageID":"msg_1","type":"tool","callID":"c_1","tool":"bash","state":{{"status":"error","input":{{"command":"pwd"}},"error":{error},"time":{{"start":1400,"end":1500}}}}}}}}"#,
            "\n"
        ),
        error = error
    );
    fs::write(outputs_dir.join("opencode-events.jsonl"), body).unwrap();
}

/// Write a denial-less OpenCode events file (final text + step finish only).
fn write_opencode_events_without_denials(outputs_dir: &Path, final_text: &str) {
    let text = serde_json::to_string(final_text).unwrap();
    fs::write(
        outputs_dir.join("opencode-events.jsonl"),
        format!(
            concat!(
                r#"{{"type":"text","timestamp":1000,"sessionID":"ses_1","part":{{"id":"p1","type":"text","text":{text}}}}}"#,
                "\n",
                r#"{{"type":"step_finish","timestamp":2000,"sessionID":"ses_1","part":{{"id":"p2","type":"step-finish","reason":"stop","tokens":{{"input":1,"output":1,"reasoning":0,"cache":{{"read":0,"write":0}}}}}}}}"#,
                "\n"
            ),
            text = text
        ),
    )
    .unwrap();
}

#[test]
fn opencode_record_runs_reports_permission_denials_from_the_event_stream() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "opencode"])
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
            write_opencode_events_with_denial(&outputs);
        } else {
            write_opencode_events_without_denials(&outputs, "Reviewed.");
        }
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["record-runs", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--workspace-dir"])
        .arg(cwd.join(".eval-magic"))
        .args(["--harness", "opencode", "--iteration", "1"])
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
    assert_eq!(reported[0]["denials"][0]["tool"], "bash");
    // The ruleset JSON is stripped to the fixed refusal prefix.
    assert_eq!(
        reported[0]["denials"][0]["reason"],
        OPENCODE_DENY_RULE_PREFIX
    );
    assert_eq!(
        reported[0]["denials"][0]["input_keys"],
        serde_json::json!(["command"])
    );

    for task in &tasks {
        assert!(
            resolve(&cwd, task["run_record_path"].as_str().unwrap()).exists(),
            "{task:?}"
        );
    }
}
