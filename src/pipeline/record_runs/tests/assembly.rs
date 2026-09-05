//! Assembling `run.json` and `timing.json` from what each harness left on disk,
//! including transcript-owned final responses and the `dispatch.json` input
//! contract.

use super::*;

#[test]
fn assembles_run_and_timing_for_every_task_from_disk() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[
            FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
            },
            FixtureTask {
                eval_id: "crash",
                condition: "without_skill",
            },
        ],
    );
    write_claude_events(&paths[0].outputs_dir, "Fixed it.");
    write_claude_events(&paths[1].outputs_dir, "Done, I think.");

    let result = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 2);
    assert_eq!(result.missing_transcript, 0);

    let run = read_run(&iter, "crash", "with_skill");
    assert_eq!(run.eval_id, "crash");
    assert_eq!(run.condition, "with_skill");
    assert_eq!(run.skill_path.as_deref(), Some("/staged/skill/SKILL.md"));
    assert_eq!(run.prompt, "Do the crash task");
    assert_eq!(run.files.len(), 1);
    assert_eq!(run.final_message, "Fixed it.");
    assert_eq!(run.tool_invocations.len(), 1);
    assert_eq!(run.tool_invocations[0].name, "Bash");
    assert_eq!(run.tool_invocations[0].ordinal, 1);
    let run_json: Value = serde_json::from_str(
        &fs::read_to_string(iter.join("eval-crash/with_skill/run.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        run_json["staged_skill_path"],
        json!("/task/.agents/skills/test-slug/SKILL.md")
    );

    assert!(
        read_run(&iter, "crash", "without_skill")
            .skill_path
            .is_none()
    );

    let timing = read_timing_value(&iter, "crash", "with_skill");
    assert_eq!(timing["total_tokens"], json!(125));
    assert_eq!(timing["duration_ms"], json!(30_000));
    assert_eq!(timing["token_source"], json!("transcript"));
    assert_eq!(timing["duration_source"], json!("transcript"));
}

#[test]
fn historical_codex_completion_without_runner_or_timestamps_keeps_duration_missing() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "historical",
            condition: "with_skill",
        }],
    );
    write_transcript_file(
        &paths[0].outputs_dir,
        "codex-events.jsonl",
        jsonl(&[
            json!({"type": "item.completed", "item": {"id": "m1", "type": "agent_message", "text": "Done."}}),
            json!({"type": "turn.completed", "usage": {"input_tokens": 2, "output_tokens": 3}}),
        ]),
    );

    record_runs(&iter, 1, Harness::resolve("codex").unwrap(), false).unwrap();

    let timing = read_timing_value(&iter, "historical", "with_skill");
    assert_eq!(timing["total_tokens"], 5);
    assert_eq!(timing["token_source"], "transcript");
    assert!(timing["duration_ms"].is_null(), "{timing}");
    assert_eq!(timing["duration_source"], "transcript");
}

/// Grading reads `run.json` and nothing else, so the record has to name the
/// tree the agent worked in — otherwise a result cannot be tied to a codebase
/// at the only granularity that matters, the individual run.
#[test]
fn carries_the_codebase_from_dispatch_task_into_each_run_record() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let cond_dir = iter.join("eval-crash").join("with_skill");
    let outputs_dir = cond_dir.join("outputs");
    fs::create_dir_all(&outputs_dir).unwrap();
    let conversation_path = outputs_dir.join("conversation.json");
    write_one_shot_completion(&conversation_path, "Do the crash task");
    write_codex_events(&outputs_dir, "Fixed it.");
    let codebase = json!({
        "kind": "git",
        "source": "https://example.com/project.git",
        "ref": "main",
        "revision": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
        "branch": "main",
        "exclude_skill_sources": true
    });
    fs::write(
        iter.join("dispatch.json"),
        serde_json::to_string_pretty(&json!({
            "run_nonce": "nonce1",
            "tasks": [{
                "eval_id": "crash",
                "condition": "with_skill",
                "skill_path": "/staged/skill/SKILL.md",
                "user_prompt": "Do the crash task",
                "files": [],
                "outputs_dir": outputs_dir.to_string_lossy(),
                "conversation_path": conversation_path.to_string_lossy(),
                "run_record_path": cond_dir.join("run.json").to_string_lossy(),
                "timing_path": cond_dir.join("timing.json").to_string_lossy(),
                "agent_description": "crash:with_skill:i1-nonce1",
                "codebase": codebase,
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    record_runs(&iter, 1, Harness::resolve("codex").unwrap(), false).unwrap();

    let recorded: Value =
        serde_json::from_str(&fs::read_to_string(cond_dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(recorded["codebase"], codebase);
}

/// Grading reads `run.json` and nothing else, so a result can only be tied to a
/// skill revision if the record carries one.
#[test]
fn carries_the_skill_source_from_dispatch_task_into_each_run_record() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let cond_dir = iter.join("eval-crash").join("with_skill");
    let outputs_dir = cond_dir.join("outputs");
    fs::create_dir_all(&outputs_dir).unwrap();
    let conversation_path = outputs_dir.join("conversation.json");
    write_one_shot_completion(&conversation_path, "Do the crash task");
    write_codex_events(&outputs_dir, "Fixed it.");
    let skill_source = json!({
        "kind": "path",
        "source": "/work/skills/mr-review",
        "resolved_path": "/work/skills/mr-review",
        "revision": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
        "branch": "main",
        "host_local": true,
        "dirty": true,
        "siblings": ["helper-skill"]
    });
    fs::write(
        iter.join("dispatch.json"),
        serde_json::to_string_pretty(&json!({
            "run_nonce": "nonce1",
            "tasks": [{
                "eval_id": "crash",
                "condition": "with_skill",
                "skill_path": "/staged/skill/SKILL.md",
                "user_prompt": "Do the crash task",
                "files": [],
                "outputs_dir": outputs_dir.to_string_lossy(),
                "conversation_path": conversation_path.to_string_lossy(),
                "run_record_path": cond_dir.join("run.json").to_string_lossy(),
                "timing_path": cond_dir.join("timing.json").to_string_lossy(),
                "agent_description": "crash:with_skill:i1-nonce1",
                "skill_source": skill_source,
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    record_runs(&iter, 1, Harness::resolve("codex").unwrap(), false).unwrap();

    let recorded: Value =
        serde_json::from_str(&fs::read_to_string(cond_dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(recorded["skill_source"], skill_source);
}

/// A run with no codebase behind it serializes exactly as it did before the
/// field existed, so historical records stay comparable.
#[test]
fn omits_the_codebase_key_when_a_task_declares_none() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "crash",
            condition: "with_skill",
        }],
    );
    write_claude_events(&paths[0].outputs_dir, "unused");

    record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();

    let recorded: Value = serde_json::from_str(
        &fs::read_to_string(iter.join("eval-crash").join("with_skill").join("run.json")).unwrap(),
    )
    .unwrap();
    assert!(recorded.get("codebase").is_none());
}

#[test]
fn carries_run_index_from_dispatch_task_into_each_run_record() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let cond_dir = iter.join("eval-crash").join("with_skill");
    let mut serialized = Vec::new();
    for k in [1u32, 2] {
        let run_dir = cond_dir.join(format!("run-{k}"));
        let outputs_dir = run_dir.join("outputs");
        fs::create_dir_all(&outputs_dir).unwrap();
        let conversation_path = outputs_dir.join("conversation.json");
        write_one_shot_completion(&conversation_path, "Do the crash task");
        write_codex_events(&outputs_dir, &format!("Fixed it in run {k}."));
        serialized.push(json!({
            "eval_id": "crash",
            "condition": "with_skill",
            "run_index": k,
            "skill_path": "/staged/skill/SKILL.md",
            "user_prompt": "Do the crash task",
            "files": [],
            "outputs_dir": outputs_dir.to_string_lossy(),
            "conversation_path": conversation_path.to_string_lossy(),
            "run_record_path": run_dir.join("run.json").to_string_lossy(),
            "timing_path": run_dir.join("timing.json").to_string_lossy(),
            "agent_description": format!("crash:with_skill:r{k}:i1-nonce1"),
        }));
    }
    fs::write(
        iter.join("dispatch.json"),
        serde_json::to_string_pretty(&json!({"run_nonce": "nonce1", "tasks": serialized})).unwrap(),
    )
    .unwrap();

    let result = record_runs(&iter, 1, Harness::resolve("codex").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 2);

    for k in [1u32, 2] {
        let raw = fs::read_to_string(cond_dir.join(format!("run-{k}")).join("run.json")).unwrap();
        let run: RunRecord = serde_json::from_str(&raw).unwrap();
        assert_eq!(run.run_index, Some(k), "wrong run_index for run-{k}");
        assert_eq!(run.final_message, format!("Fixed it in run {k}."));
    }
}

#[test]
fn assembles_codex_records_from_each_tasks_events() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "crash",
            condition: "with_skill",
        }],
    );
    write_codex_events(&paths[0].outputs_dir, "Codex final.");

    let result = record_runs(&iter, 1, Harness::resolve("codex").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 1);
    assert_eq!(result.missing_transcript, 0);

    let run = read_run(&iter, "crash", "with_skill");
    assert_eq!(run.final_message, "Codex final.");
    assert_eq!(
        serde_json::to_value(&run.tool_invocations).unwrap(),
        json!([{"name": "command_execution", "ordinal": 1, "args": {"command": "bun test"}, "result": "ok"}])
    );

    let timing = read_timing_value(&iter, "crash", "with_skill");
    assert_eq!(
        timing,
        json!({
            "total_tokens": 40,
            "duration_ms": 30_000,
            "token_source": "transcript",
            "duration_source": "transcript"
        })
    );
}

#[test]
fn records_codex_final_agent_message_from_the_transcript() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "crash",
            condition: "with_skill",
        }],
    );
    write_codex_events(&paths[0].outputs_dir, "Closing summary from Codex.");

    let result = record_runs(&iter, 1, Harness::resolve("codex").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 1);
    assert_eq!(
        read_run(&iter, "crash", "with_skill").final_message,
        "Closing summary from Codex."
    );
}

#[test]
fn assembles_claude_records_from_each_tasks_events() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "crash",
            condition: "with_skill",
        }],
    );
    write_claude_events(&paths[0].outputs_dir, "Closing summary.");

    let result = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 1);
    assert_eq!(result.missing_transcript, 0);

    let run = read_run(&iter, "crash", "with_skill");
    assert_eq!(run.final_message, "Closing summary.");
    assert_eq!(
        serde_json::to_value(&run.tool_invocations).unwrap(),
        json!([{"name": "Bash", "ordinal": 1, "args": {"command": "bun test"}, "result": "ok"}])
    );
    let timing = read_timing_value(&iter, "crash", "with_skill");
    assert_eq!(
        timing,
        json!({
            "total_tokens": 125,
            "duration_ms": 30_000,
            "token_source": "transcript",
            "duration_source": "transcript"
        })
    );
}

#[test]
fn records_claude_result_final_text_from_the_transcript() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    let paths = write_iteration(
        &iter,
        &[FixtureTask {
            eval_id: "crash",
            condition: "with_skill",
        }],
    );
    write_claude_events(&paths[0].outputs_dir, "Closing summary from claude -p.");

    let result = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap();
    assert_eq!(result.recorded, 1);
    assert_eq!(
        read_run(&iter, "crash", "with_skill").final_message,
        "Closing summary from claude -p."
    );
}

#[test]
fn errors_when_dispatch_json_is_absent() {
    let root = TempDir::new().unwrap();
    let iter = dirs(&root);
    // Hand-authored/operator runs have no dispatch.json — the manual path owns them.
    let err = record_runs(&iter, 1, Harness::resolve("claude-code").unwrap(), false).unwrap_err();
    assert!(
        err.to_string().contains("dispatch.json"),
        "error was: {err}"
    );
}
