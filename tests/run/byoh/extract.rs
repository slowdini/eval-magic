//! Declarative transcript extraction for a descriptor-only harness.

use super::*;

/// A stream no built-in parser understands still yields full transcript ingest
/// and a session-surface report from descriptor data alone.
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

[transcript.extract.session_surface]
where = { kind = "roster" }
skills_field = "surface.skills"
plugins_field = "surface.plugins"
plugin_name_field = "label"
plugin_id_field = "id"
plugin_version_field = "release"

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
                r#"{"kind":"roster","surface":{"skills":["cool:review"],"plugins":[{"label":"cool","id":"cool@vendor","release":"1.2.3"}]}}"#,
                "\n",
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

    let surface = read_json(&iteration_dir(&cwd).join("session-surface.json"));
    assert_eq!(surface["tasks_without_evidence"], 0, "{surface}");
    for task in surface["tasks"].as_array().unwrap() {
        let reported = &task["rounds"][0]["surface"];
        assert_eq!(
            reported["advertised_skills"],
            serde_json::json!(["cool:review"])
        );
        assert_eq!(
            reported["loaded_plugins"],
            serde_json::json!([{
                "name": "cool",
                "source": "cool@vendor",
                "version": "1.2.3"
            }])
        );
    }
}
