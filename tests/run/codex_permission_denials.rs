//! Codex permission-denial capture and record-runs integration.

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

#[test]
fn codex_record_runs_reports_permission_denials_from_stderr() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "codex"])
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
        fs::write(
            outputs.join("codex-events.jsonl"),
            concat!(
                r#"{"type":"thread.started","thread_id":"thread_1","timestamp":"2026-07-30T06:00:00Z"}"#,
                "\n",
                r#"{"type":"item.completed","timestamp":"2026-07-30T06:00:01Z","item":{"id":"item_1","type":"agent_message","text":"Reviewed."}}"#,
                "\n",
                r#"{"type":"turn.completed","timestamp":"2026-07-30T06:00:02Z","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":2}}"#,
                "\n"
            ),
        )
        .unwrap();
        if task["condition"] == "with_skill" {
            fs::write(
                outputs.join("codex-stderr.log"),
                concat!(
                    "2026-07-30T06:00:01Z ERROR codex_core::tools::router: ",
                    "error=exec_command failed for `/bin/zsh -lc pwd`: CreateProcess { ",
                    "message: \"Rejected(\\\"`/bin/zsh -lc pwd` rejected: ",
                    "This command requires approval\\\")\" }\n"
                ),
            )
            .unwrap();
        }
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["record-runs", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--workspace-dir"])
        .arg(cwd.join(".eval-magic"))
        .args(["--harness", "codex", "--iteration", "1"])
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
    assert_eq!(reported[0]["denials"][0]["tool"], "Bash");
    assert_eq!(
        reported[0]["denials"][0]["reason"],
        "This command requires approval"
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
