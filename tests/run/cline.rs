//! Cline-harness behavior: `.cline/skills` staging with the default
//! (underscore-preserving) slug template, the auto-armed write guard (the
//! `cline-plugin` engine's staged project plugin), the `cline-skills` shadow
//! preflight, and `cline-json` transcript ingest of the `cline --json` NDJSON
//! stream (`agent_event` records plus the terminal `run_result`).

use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;

/// The slug the default template gives (iteration 1, with_skill, mr-review).
/// Underscores survive: Cline 3.0.52 does not enforce the Agent Skills naming
/// spec (verified in docs/cline-notes.md), so no slug capability is needed.
const CLINE_SLUG: &str = "slow-powers-eval-1-with_skill__mr-review";

#[test]
fn cline_no_stage_keeps_inline_fallback() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
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
            "cline",
            "--no-stage",
            "--dry-run",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(dispatch["harness"], "cline");
    assert_eq!(conditions["harness"], "cline");
    assert!(!cwd.join(".cline/skills").exists());
}

#[test]
fn cline_stages_repo_local_skills_under_cline_dir() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let helper = skill_dir.join("release-notes");
    fs::create_dir_all(&helper).unwrap();
    fs::write(
        helper.join("SKILL.md"),
        "---\nname: release-notes\ndescription: draft release notes\n---\n\nnotes\n",
    )
    .unwrap();

    // Every enhancement is wired for Cline now, so none of the fallback
    // preflight warnings may fire on a dry run.
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
            "cline",
            "--dry-run",
        ])
        .assert()
        .success()
        .stderr(
            contains("declares no write guard")
                .not()
                .and(contains("declares no transcript parser").not())
                .and(contains("cannot tell a permission-denied").not())
                .and(contains("declares no skills_dir").not())
                .and(contains("declares no model flag").not()),
        );

    // The skill-under-test stages under the underscore-preserving slug with
    // its frontmatter rewritten; the sibling skill keeps its natural name.
    let cline_skills = cli_env_dir(&cwd, "g1", "with_skill").join(".cline/skills");
    assert!(
        read_str(&cline_skills.join(CLINE_SLUG).join("SKILL.md"))
            .contains(&format!("name: {CLINE_SLUG}")),
        "staged SKILL.md frontmatter name should be rewritten to the slug"
    );
    assert!(cline_skills.join("release-notes").join("SKILL.md").exists());

    // The control arm stays skill-absent.
    let control_skills = cli_env_dir(&cwd, "g1", "without_skill").join(".cline/skills");
    assert!(!control_skills.join(CLINE_SLUG).exists());
}

/// Resolve a dispatch.json path (they may be cwd-relative) against `cwd`.
fn resolve(cwd: &std::path::Path, path: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// The tasks[] array from the iteration's dispatch.json.
fn dispatch_tasks(cwd: &std::path::Path) -> Vec<serde_json::Value> {
    read_json(&iteration_dir(cwd).join("dispatch.json"))["tasks"]
        .as_array()
        .expect("dispatch.json carries tasks[]")
        .clone()
}

/// The observed 3.0.52 stream shape, minimized: one self-contained tool call,
/// one assistant text block, a lifecycle line, and the terminal `run_result`.
const CLINE_EVENTS: &str = concat!(
    r#"{"ts":"2026-08-11T19:29:50.000Z","type":"agent_event","event":{"type":"content_start","contentType":"tool","toolCallId":"run_commands_0","toolName":"run_commands","input":{"commands":["echo hi"]}}}"#,
    "\n",
    r#"{"ts":"2026-08-11T19:29:51.000Z","type":"agent_event","event":{"type":"content_end","contentType":"tool","toolCallId":"run_commands_0","toolName":"run_commands","output":[{"query":"echo hi","result":"hi\n","success":true}]}}"#,
    "\n",
    r#"{"ts":"2026-08-11T19:29:52.000Z","type":"agent_event","event":{"type":"content_end","contentType":"text","text":"Done."}}"#,
    "\n",
    r#"{"ts":"2026-08-11T19:29:53.000Z","type":"hook_event","hookEventName":"agent_end","agentId":"agent_1","taskId":"conv_1","parentAgentId":null}"#,
    "\n",
    r#"{"ts":"2026-08-11T19:29:54.000Z","type":"run_result","finishReason":"completed","iterations":1,"usage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":40,"cacheWriteTokens":0,"totalCost":0.001},"aggregateUsage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":40,"cacheWriteTokens":0,"totalCost":0.001},"durationMs":12345,"text":"Done."}"#,
    "\n",
);

#[test]
fn cline_default_run_auto_arms_guard() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // No guard flag: cline declares guard support, so the bare run arms it
    // (enhancements are provided, not opted into) and prints the armed banner
    // naming the plugin file.
    let assert = skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--harness",
            "cline",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("guard: armed"),
        "the run plan reports the armed guard: {stdout}"
    );
    assert!(
        stdout.contains(".cline/plugins/slow-powers-eval-guard/index.js"),
        "the armed banner names the plugin file: {stdout}"
    );

    assert!(
        cli_env_dir(&cwd, "g1", "with_skill")
            .join(".cline/plugins/slow-powers-eval-guard/index.js")
            .exists(),
        "cline guard plugin staged in the env"
    );
}

/// A live copy of the eval skill in either verified global root (the native
/// `$CLINE_DIR/skills` defaulting to `~/.cline/skills`, or the cross-harness
/// `~/.agents/skills`) must surface in the shadow preflight; Cline reads only
/// the dispatch cwd's `.cline/skills` (3.0.53 probe: no ancestor walk), so no
/// project roots are scanned.
#[test]
fn cline_warns_when_live_global_skill_shadows_staged_skill() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let fake_home = tmp.path().join("home");
    // Native root: $CLINE_DIR/skills (pinned to a temp dir so the operator's
    // real ~/.cline/skills stays out of the test).
    let cline_home = tmp.path().join("cline-home");
    let native_skill = cline_home.join("skills/mr-review");
    fs::create_dir_all(&native_skill).unwrap();
    fs::write(
        native_skill.join("SKILL.md"),
        "---\nname: mr-review\ndescription: installed copy\n---\n\nlive\n",
    )
    .unwrap();
    // Cross-harness root: ~/.agents/skills (cline skill install lands here).
    let agents_skill = fake_home.join(".agents/skills/mr-review");
    fs::create_dir_all(&agents_skill).unwrap();
    fs::write(
        agents_skill.join("SKILL.md"),
        "---\nname: mr-review\ndescription: codex-side copy\n---\n\nlive\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .env("HOME", &fake_home)
        .env("CLINE_DIR", &cline_home)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "cline", "--dry-run"])
        .assert()
        .success()
        .stderr(contains("Skill-shadow preflight"))
        .stderr(contains("cross-harness"));

    let report = read_json(&iteration_dir(&cwd).join("plugin-shadow.json"));
    assert_eq!(report["schema_version"], 3);
    assert_eq!(report["findings"][0]["skill_name"], "mr-review");
    let sources = report["findings"][0]["sources"].as_array().unwrap();
    let mut namespaces: Vec<&str> = sources
        .iter()
        .filter(|source| source["origin"] == "live")
        .map(|source| source["root"]["namespace"].as_str().unwrap())
        .collect();
    namespaces.sort_unstable();
    assert_eq!(namespaces, ["agents", "cline"], "{report}");
    let relations: Vec<&str> = sources
        .iter()
        .filter(|source| source["origin"] == "live")
        .map(|source| source["root"]["relation"].as_str().unwrap())
        .collect();
    assert!(relations.contains(&"native"), "{report}");
    assert!(relations.contains(&"cross-harness"), "{report}");
}

/// Ingest reads the declared `cline-json` parser tier: tool invocations get
/// flattened top-level args (`run_commands`' `commands` array joins into
/// `command`) with the result attached from the paired `content_end`, and
/// timing backfills from `run_result` (cached input subtracted, matching
/// codex accounting).
#[test]
fn cline_ingest_extracts_summary_from_events_stream() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

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
            "cline",
        ])
        .assert()
        .success();

    // Simulate dispatches that emit the harness's own stream.
    for task in dispatch_tasks(&cwd) {
        let outputs = resolve(&cwd, task["outputs_dir"].as_str().unwrap());
        fs::create_dir_all(&outputs).unwrap();
        fs::write(outputs.join("final-message.md"), "Done.\n").unwrap();
        fs::write(outputs.join("cline-events.jsonl"), CLINE_EVENTS).unwrap();
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "cline",
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
        assert_eq!(invocations[0]["name"], "run_commands");
        assert_eq!(
            invocations[0]["args"],
            serde_json::json!({"command": "echo hi"})
        );
        assert_eq!(invocations[0]["result"], serde_json::json!("hi\n"));
        assert_eq!(record["final_message"], "Done.");

        let timing = read_json(&resolve(&cwd, task["timing_path"].as_str().unwrap()));
        assert_eq!(timing["total_tokens"], 80, "{timing}");
        assert_eq!(timing["duration_ms"], 12_345, "{timing}");
    }
}
