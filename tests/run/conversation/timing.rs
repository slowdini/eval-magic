//! Cross-harness runner timing and timing-artifact handoff.

use super::{ONE_SHOT_EVALS, dispatch_one, prepare_one_shot_run, stub_exec_template};
use crate::helpers::*;
use std::fs;
use std::path::Path;

/// Dispatch timing belongs to the shared runner, so every built-in harness and
/// a runner-ready descriptor get the same completion/timing contract even when
/// their native transcripts disagree about duration.
#[test]
fn runner_duration_is_persisted_for_every_builtin_harness_and_byoh() {
    struct Case {
        label: &'static str,
        filename: &'static str,
        events: &'static str,
        byoh: bool,
        plan_mode: bool,
    }

    let cases = [
        Case {
            label: "claude-code",
            filename: "claude-events.jsonl",
            events: r#"{"type":"result","subtype":"success","is_error":false,"result":"Done.","duration_ms":999999,"usage":{"input_tokens":2,"output_tokens":3}}
"#,
            byoh: false,
            plan_mode: true,
        },
        Case {
            label: "codex",
            filename: "codex-events.jsonl",
            events: r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"Done."}}
{"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}
"#,
            byoh: false,
            plan_mode: false,
        },
        Case {
            label: "cline",
            filename: "cline-events.jsonl",
            events: r#"{"type":"run_result","finishReason":"completed","iterations":1,"usage":{"inputTokens":2,"outputTokens":3,"cacheReadTokens":0,"cacheWriteTokens":0},"aggregateUsage":{"inputTokens":2,"outputTokens":3,"cacheReadTokens":0,"cacheWriteTokens":0},"durationMs":999999,"text":"Done."}
"#,
            byoh: false,
            plan_mode: false,
        },
        Case {
            label: "opencode",
            filename: "opencode-events.jsonl",
            events: r#"{"type":"text","timestamp":1,"sessionID":"ses_1","part":{"id":"p1","type":"text","text":"Done."}}
{"type":"step_finish","timestamp":1000000,"sessionID":"ses_1","part":{"id":"p2","type":"step-finish","reason":"stop","tokens":{"input":2,"output":3,"reasoning":0,"cache":{"read":0,"write":0}}}}
"#,
            byoh: false,
            plan_mode: true,
        },
        Case {
            label: "cool-custom-harness",
            filename: "cool-events.jsonl",
            events: r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"Done."}}
{"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}
"#,
            byoh: true,
            plan_mode: false,
        },
    ];

    for case in cases {
        let tmp = tempfile::TempDir::new().unwrap();
        let (skill_dir, cwd) = setup(tmp.path(), ONE_SHOT_EVALS);
        if case.byoh {
            let descriptor_dir = cwd.join(".eval-magic/harnesses");
            fs::create_dir_all(&descriptor_dir).unwrap();
            fs::write(
                descriptor_dir.join("cool.toml"),
                RUNNER_READY_NO_RESUME_DESCRIPTOR,
            )
            .unwrap();
        }
        prepare_one_shot_run(&skill_dir, &cwd, case.label);
        let capability_slots = if case.plan_mode {
            "# {mode_args}{model_arg}"
        } else {
            "# {model_arg}"
        };
        let command = format!(
            "\"{}\" __fixture --sleep-ms 15 --text {} --write \
             \"<outputs_dir>/{}\" {capability_slots}",
            env!("CARGO_BIN_EXE_eval-magic"),
            shell_quote(case.events),
            case.filename
        );
        stub_exec_template(&cwd, &command);

        dispatch_one(&skill_dir, &cwd, case.label, 0, false)
            .assert()
            .success();

        let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
        let task = &dispatch["tasks"][0];
        let conversation = read_json(Path::new(task["conversation_path"].as_str().unwrap()));
        let duration = conversation["duration_ms"]
            .as_u64()
            .unwrap_or_else(|| panic!("{} has no runner duration: {conversation}", case.label));
        assert!(duration >= 10, "{} duration was {duration}ms", case.label);

        skill_eval()
            .current_dir(&cwd)
            .args(["record-runs", "--skill-dir"])
            .arg(&skill_dir)
            .args([
                "--skill",
                "mr-review",
                "--iteration",
                "1",
                "--harness",
                case.label,
            ])
            .assert()
            .success();

        let timing = read_json(Path::new(task["timing_path"].as_str().unwrap()));
        assert_eq!(timing["duration_ms"], duration, "{}: {timing}", case.label);
        assert_eq!(
            timing["duration_source"], "runner",
            "{}: {timing}",
            case.label
        );
        assert_eq!(
            timing["token_source"], "transcript",
            "{}: {timing}",
            case.label
        );
        let run = read_json(Path::new(task["run_record_path"].as_str().unwrap()));
        assert!(
            run["conversation"].get("duration_ms").is_none(),
            "{} run.json duplicated canonical timing: {run}",
            case.label
        );
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
