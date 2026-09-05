use super::*;

#[test]
fn codex_exact_path_access_grades_each_treatment_member_without_meta_judges() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = prepare(
        tmp.path(),
        multi_evals(),
        &["--mode", "new-skill", "--harness", "codex"],
    );

    let iteration = iteration_dir(&cwd);
    let dispatch = read_json(&iteration.join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        let events = if task["condition"] == "with_skill" {
            let staged_path = task["skills"][1]["staged_skill_path"]
                .as_str()
                .expect("each Codex treatment member records its exact staged path");
            include_str!("../../fixtures/codex/skill-access-0.152.1.jsonl")
                .replace("__STAGED_SKILL_PATH__", staged_path)
        } else {
            concat!(
                r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"done"}}"#,
                "\n"
            )
            .to_string()
        };
        write_task_transcript(&cwd, task, "codex-events.jsonl", &events);
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

    let tasks = read_json(&iteration.join("judge-tasks.json"));
    let meta = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|task| task["is_meta"] == true)
        .collect::<Vec<_>>();
    assert!(
        meta.is_empty(),
        "Codex exact-path evidence is graded locally"
    );

    let responses = iteration.join("eval-e1/with_skill/judge-responses");
    let first = read_json(&responses.join("__skill_invoked__skill-1.json"));
    let second = read_json(&responses.join("__skill_invoked__skill-2.json"));
    assert_eq!(
        first["passed"], false,
        "final-message wording is not evidence"
    );
    assert_eq!(second["passed"], true, "the exact staged path was read");
    for response in [&first, &second] {
        assert_eq!(response["grader"], "transcript_check");
        assert_eq!(response["confidence"], 1.0);
        assert!(response["evidence"].as_str().unwrap().contains("access"));
    }
}

#[test]
fn descriptor_without_deterministic_evidence_retains_one_labeled_fallback_per_member() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &multi_evals().to_string());
    add_skill(&skill_dir, "supporting-skill");
    let descriptor_dir = cwd.join(".eval-magic/harnesses");
    fs::create_dir_all(&descriptor_dir).unwrap();
    fs::write(
        descriptor_dir.join("fallback.toml"),
        r#"label = "fallback"
skills_dir = ".agents/skills"
config_dirs = [".agents"]

[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "fallback-events.jsonl"
parser = "codex-items"
surfaces_skill_invocation = false

[dispatch]
exec_template = "fallback --cd <eval-root> <dispatch_prompt_path> > <outputs_dir>/fallback-events.jsonl"
"#,
    )
    .unwrap();
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "fallback", "--dry-run"])
        .assert()
        .success();

    let iteration = iteration_dir(&cwd);
    let dispatch = read_json(&iteration.join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        let run_path = Path::new(task["run_record_path"].as_str().unwrap());
        fs::write(
            run_path,
            serde_json::to_vec_pretty(&json!({
                "eval_id": task["eval_id"],
                "condition": task["condition"],
                "skill_path": task["skill_path"],
                "skills": [],
                "prompt": task["user_prompt"],
                "files": task["files"],
                "final_message": "done",
                "tool_invocations": [],
                "total_tokens": null,
                "duration_ms": null
            }))
            .unwrap(),
        )
        .unwrap();
    }
    skill_eval()
        .current_dir(&cwd)
        .args(["grade", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "fallback",
            "--iteration",
            "1",
        ])
        .assert()
        .success();

    let tasks = read_json(&iteration.join("judge-tasks.json"));
    let meta = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|task| task["is_meta"] == true)
        .collect::<Vec<_>>();
    assert_eq!(meta.len(), 2, "{tasks}");
    assert_eq!(meta[0]["skill_name"], "mr-review");
    assert_eq!(meta[1]["skill_name"], "supporting-skill");
    for task in meta {
        let rubric = task["rubric"].as_str().unwrap();
        assert!(rubric.contains("behavioral-influence fallback"), "{rubric}");
        assert!(
            rubric.contains("does not prove native invocation or access"),
            "{rubric}"
        );
    }
}
