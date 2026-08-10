//! Cross-harness runtime identity for staged subjects in shadow reports.

use crate::helpers::*;
use std::fs;
use std::path::{Path, PathBuf};

fn write_live_skill(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::write(
        path.join("SKILL.md"),
        "---\nname: mr-review\ndescription: live copy\n---\n",
    )
    .unwrap();
}

#[test]
fn staged_runtime_ids_match_staged_slugs_for_every_builtin_harness() {
    for harness in ["claude-code", "codex", "opencode"] {
        let tmp = tempfile::TempDir::new().unwrap();
        let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
        let fake_home = tmp.path().join("home");
        let mut command = skill_eval();
        command
            .current_dir(&cwd)
            .args(["run", "--skill-dir"])
            .arg(&skill_dir)
            .args(["--skill", "mr-review", "--harness", harness, "--dry-run"]);

        match harness {
            "claude-code" => {
                let config = tmp.path().join("claude-config");
                write_live_skill(&config.join("skills/mr-review"));
                command.env("CLAUDE_CONFIG_DIR", config);
            }
            "codex" => {
                write_live_skill(&fake_home.join(".agents/skills/different-folder"));
                command
                    .env("HOME", &fake_home)
                    .env("CODEX_HOME", tmp.path().join("codex-home"));
            }
            "opencode" => {
                write_live_skill(&fake_home.join(".claude/skills/different-folder"));
                command
                    .env("HOME", &fake_home)
                    .env("XDG_CONFIG_HOME", fake_home.join("xdg"))
                    .env("OPENCODE_CONFIG_DIR", tmp.path().join("opencode-config"));
            }
            _ => unreachable!(),
        }
        command.assert().success();

        let iteration = iteration_dir(&cwd);
        let report = read_json(&iteration.join("plugin-shadow.json"));
        let conditions = read_json(&iteration.join("conditions.json"));
        let staged_slug = conditions["conditions"][0]["staged_skill_slug"]
            .as_str()
            .unwrap();
        let staged = report["findings"][0]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|source| source["origin"] == "staged")
            .collect::<Vec<_>>();
        assert!(!staged.is_empty(), "{harness} recorded no staged source");
        assert!(
            staged
                .iter()
                .all(|source| source["runtime_id"] == staged_slug),
            "{harness} did not use the staged slug as the runtime id"
        );
        assert!(staged.iter().all(|source| {
            source["appearances"]
                .as_array()
                .unwrap()
                .iter()
                .all(|appearance| appearance["resolution"] == "selected")
        }));
    }
}

/// Claude Code reports a staged subject under its directory slug. Seeing that
/// slug without the global natural name therefore refutes the live source.
#[test]
fn claude_refutes_a_live_skill_when_only_the_staged_slug_is_reported() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let config = tmp.path().join("config");
    write_live_skill(&config.join("skills/mr-review"));

    skill_eval()
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &config)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "claude-code"])
        .assert()
        .success();

    let iteration = iteration_dir(&cwd);
    let dispatch = read_json(&iteration.join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        let outputs = PathBuf::from(task["outputs_dir"].as_str().unwrap());
        fs::create_dir_all(&outputs).unwrap();
        let skills = task["staged_skill_slug"]
            .as_str()
            .into_iter()
            .collect::<Vec<_>>();
        let events = [
            serde_json::json!({"type": "system", "subtype": "hook_started"}),
            serde_json::json!({
                "type": "system",
                "subtype": "init",
                "session_id": "s1",
                "plugins": [],
                "skills": skills,
            }),
            serde_json::json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "result": "done",
                "duration_ms": 5,
                "usage": {"input_tokens": 1, "output_tokens": 1},
            }),
        ]
        .into_iter()
        .map(|event| event.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        fs::write(outputs.join("claude-events.jsonl"), format!("{events}\n")).unwrap();
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["record-runs", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "claude-code"])
        .assert()
        .success();

    let artifact = read_json(&iteration.join("plugin-shadow.json"));
    let finding = &artifact["findings"][0];
    assert_eq!(finding["resolved_severity"], "isolated");
    assert_eq!(artifact["verification"]["refuted_findings"], 1);

    let sources = finding["sources"].as_array().unwrap();
    let live = sources
        .iter()
        .find(|source| source["origin"] == "live")
        .unwrap();
    let staged = sources
        .iter()
        .find(|source| source["origin"] == "staged")
        .unwrap();
    let conditions = read_json(&iteration.join("conditions.json"));
    let staged_slug = conditions["conditions"][0]["staged_skill_slug"]
        .as_str()
        .unwrap();
    assert_eq!(live["runtime_id"], "mr-review");
    assert_eq!(staged["runtime_id"], staged_slug);
    for source in [live, staged] {
        assert!(
            source["appearances"]
                .as_array()
                .unwrap()
                .iter()
                .all(|appearance| appearance["resolution"] == "selected")
        );
    }
}
