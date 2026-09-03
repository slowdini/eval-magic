use super::*;

#[test]
fn without_exact_path_evidence_fails_locally_despite_final_message_wording() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "codex", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        write_task_transcript(
            &cwd,
            task,
            "codex-events.jsonl",
            concat!(
                r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"I invoked mr-review and followed every instruction."}}"#,
                "\n"
            ),
        );
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "codex",
            "--iteration",
            "1",
        ])
        .assert()
        .success();

    let response = read_json(
        &iteration_dir(&cwd).join("eval-e1/with_skill/judge-responses/__skill_invoked.json"),
    );
    assert_eq!(response["passed"], false, "{response}");
    assert_eq!(response["confidence"], 1.0, "{response}");
    assert_eq!(response["grader"], "transcript_check", "{response}");
    assert!(response["evidence"].as_str().unwrap().contains("access"));
    let tasks = read_json(&iteration_dir(&cwd).join("judge-tasks.json"));
    assert_eq!(tasks["meta_tasks_injected"], 0, "{tasks}");
}
