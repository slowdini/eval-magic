//! OpenCode-harness behavior: `.opencode/skills` staging, slug sanitization,
//! native `<available_skills>` dispatch rendering, plan-mode approximation, and
//! the `--guard` warn-and-continue fallback. Characterization tests pinning
//! current behavior so the run-mode refactor stays behavior-preserving.

use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use std::path::Path;

/// The sanitized slug the staged skill-under-test gets for `(iteration 1,
/// with_skill, mr-review)`: underscores in the condition become hyphens.
const OPENCODE_SLUG: &str = "slow-powers-eval-1-with-skill-mr-review";

#[test]
fn opencode_no_stage_keeps_inline_fallback() {
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
            "opencode",
            "--no-stage",
            "--dry-run",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(dispatch["harness"], "opencode");
    assert_eq!(conditions["harness"], "opencode");
    assert!(!cwd.join(".claude/skills").exists());
    assert!(!cwd.join(".agents/skills").exists());
    assert!(!cwd.join(".opencode/skills").exists());
}

#[test]
fn opencode_stages_repo_local_skills_under_opencode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let helper = skill_dir.join("release-notes");
    fs::create_dir_all(&helper).unwrap();
    fs::write(
        helper.join("SKILL.md"),
        "---\nname: release-notes\ndescription: draft release notes\n---\n\nnotes\n",
    )
    .unwrap();
    fs::write(helper.join("helper.md"), "helper guidance").unwrap();

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
            "opencode",
            "--dry-run",
        ])
        .assert()
        .success();

    // OpenCode rides Cli dispatch → per-(group, condition) envs; the skill stages
    // into the with_skill env.
    let opencode_skills = cli_env_dir(&cwd, "g1", "with_skill").join(".opencode/skills");
    assert!(
        read_str(&opencode_skills.join(OPENCODE_SLUG).join("SKILL.md"))
            .contains(&format!("name: {OPENCODE_SLUG}"))
    );
    assert_eq!(
        read_str(&opencode_skills.join("release-notes/helper.md")),
        "helper guidance"
    );
    assert!(!opencode_skills.join("release-notes/evals").exists());
    assert!(!cwd.join(".claude/skills").exists());
    assert!(!cwd.join(".agents/skills").exists());

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["condition"] == "with_skill")
        .unwrap();
    let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
    // OpenCode's native skill surface: an <available_skills> XML block. The
    // skill-under-test is advertised under its staged slug — OpenCode's skill
    // tool lists frontmatter names, and staging rewrites the frontmatter `name:`
    // to the slug (siblings keep their natural names).
    assert!(prompt.contains("<available_skills>"));
    assert!(prompt.contains("</available_skills>"));
    assert!(prompt.contains(&format!("<name>{OPENCODE_SLUG}</name>")));
    assert!(!prompt.contains("<name>mr-review</name>"));
    assert!(prompt.contains("<description>review merge requests</description>"));
    assert!(prompt.contains("<name>release-notes</name>"));
    assert!(prompt.contains("<description>draft release notes</description>"));
    // Neutral slug-disambiguation framing for OpenCode points at the real identifier.
    assert!(prompt.contains(&format!("identifier `{OPENCODE_SLUG}`")));
    assert!(prompt.contains("as an OpenCode skill"));
    // Must not leak the Claude Code or Codex skill surfaces.
    assert!(!prompt.contains("The following skills are available for use with the Skill tool:"));
    assert!(!prompt.contains("## Skills"));
}

#[test]
fn opencode_plan_mode_injects_profile_and_records_flag() {
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
            "opencode",
            "--plan-mode",
            "--dry-run",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(dispatch["plan_mode"], true);
    for task in dispatch["tasks"].as_array().unwrap() {
        let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
        if task["condition"] == "with_skill" {
            assert!(prompt.contains("<available_skills>"));
        }
        assert!(prompt.contains("<system-reminder>"));
        // Shared, harness-agnostic profile: same text every harness sees.
        assert!(prompt.contains("Plan mode is active"));
        assert!(!prompt.contains("ExitPlanMode"));
    }
}

#[test]
fn opencode_guard_warns_and_continues_unguarded() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // The #126 model: an undeclared enhancement warns naming its fallback and
    // the run proceeds — it never rejects.
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
            "opencode",
            "--guard",
        ])
        .assert()
        .success()
        .stderr(
            contains("declares no write guard")
                .and(contains("detect-stray-writes"))
                .and(contains("Unsupported for --harness").not()),
        );

    // The run was built (staged, dispatch written) — just without a guard.
    assert!(iteration_dir(&cwd).exists());
    let with_skill_env = cli_env_dir(&cwd, "g1", "with_skill");
    assert!(
        with_skill_env.join(".opencode/skills").exists(),
        "staging proceeded"
    );
    assert!(
        !with_skill_env
            .join(".opencode/skills/.slow-powers-eval-guard.json")
            .exists(),
        "no guard marker was installed"
    );
}

#[test]
fn opencode_default_run_warns_unguarded_without_the_flag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // No guard flag: auto-arm can't apply (no declared guard), so the run
    // proceeds unguarded with a warning naming the fallback and the opt-out.
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
            "opencode",
        ])
        .assert()
        .success()
        .stderr(
            contains("declares no write guard")
                .and(contains("detect-stray-writes"))
                .and(contains("--no-guard")),
        );

    assert!(
        !cli_env_dir(&cwd, "g1", "with_skill")
            .join(".opencode/skills/.slow-powers-eval-guard.json")
            .exists(),
        "no guard marker was installed"
    );
}

#[test]
fn opencode_rejects_invalid_stage_name() {
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
            "opencode",
            "--stage-name",
            "Bad_Name",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(contains("OpenCode skill name \"Bad_Name\" is invalid"));
}

/// Resolve a dispatch.json path field (absolute, or relative to the run cwd).
fn resolve(cwd: &Path, path: &str) -> std::path::PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// The tasks[] array from the iteration's dispatch.json.
fn dispatch_tasks(cwd: &Path) -> Vec<serde_json::Value> {
    read_json(&iteration_dir(cwd).join("dispatch.json"))["tasks"]
        .as_array()
        .expect("dispatch.json carries tasks[]")
        .clone()
}

#[test]
fn opencode_ingest_parses_events_and_code_checks_the_skill_invocation() {
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
            "opencode",
        ])
        .assert()
        .success();

    // Simulate `opencode run --format json` dispatches: a bash call, the staged
    // skill loaded through the native `skill` tool, two text parts, and a
    // step_finish token report. No final-message.md — the transcript's last
    // text is the final-message fallback.
    for task in dispatch_tasks(&cwd) {
        let outputs = resolve(&cwd, task["outputs_dir"].as_str().unwrap());
        fs::create_dir_all(&outputs).unwrap();
        let slug_line = format!(
            r#"{{"type":"tool_use","timestamp":3000,"sessionID":"ses_1","part":{{"id":"p3","type":"tool","tool":"skill","state":{{"status":"completed","input":{{"name":"{OPENCODE_SLUG}"}},"output":"<skill/>","title":"skill","metadata":{{}},"time":{{"start":2900,"end":3000}}}}}}}}"#
        );
        fs::write(
            outputs.join("opencode-events.jsonl"),
            [
                r#"{"type":"step_start","timestamp":1000,"sessionID":"ses_1","part":{"id":"p1","type":"step-start"}}"#.to_string(),
                r#"{"type":"tool_use","timestamp":2000,"sessionID":"ses_1","part":{"id":"p2","type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"ls"},"output":"ok","title":"ls","metadata":{},"time":{"start":1900,"end":2000}}}}"#.to_string(),
                slug_line,
                r#"{"type":"text","timestamp":4000,"sessionID":"ses_1","part":{"id":"p4","type":"text","text":"First."}}"#.to_string(),
                r#"{"type":"text","timestamp":5000,"sessionID":"ses_1","part":{"id":"p5","type":"text","text":"Final."}}"#.to_string(),
                r#"{"type":"step_finish","timestamp":6000,"sessionID":"ses_1","part":{"id":"p6","type":"step-finish","reason":"stop","cost":0.002,"tokens":{"input":100,"output":20,"reasoning":5,"cache":{"read":75,"write":0}}}}"#.to_string(),
            ]
            .join("\n")
                + "\n",
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
            "opencode",
            "--iteration",
            "1",
        ])
        .assert()
        .success()
        .stderr(contains("no transcript parser").not());

    for task in dispatch_tasks(&cwd) {
        let record = read_json(&resolve(&cwd, task["run_record_path"].as_str().unwrap()));
        let invocations = record["tool_invocations"].as_array().unwrap();
        assert_eq!(
            invocations
                .iter()
                .map(|i| i["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["bash", "skill"],
            "{record}"
        );
        assert_eq!(
            invocations[1]["args"],
            serde_json::json!({"name": OPENCODE_SLUG})
        );
        assert_eq!(
            record["final_message"], "Final.",
            "the transcript's last text is the final-message fallback: {record}"
        );

        let timing = read_json(&resolve(&cwd, task["timing_path"].as_str().unwrap()));
        assert_eq!(timing["total_tokens"], 125, "{timing}");
        assert_eq!(timing["duration_ms"], 5_000, "{timing}");

        if task["condition"] == "with_skill" {
            let meta = read_json(
                &resolve(&cwd, task["run_record_path"].as_str().unwrap())
                    .parent()
                    .unwrap()
                    .join("judge-responses/__skill_invoked.json"),
            );
            assert_eq!(meta["passed"], true, "{meta}");
            assert_eq!(meta["grader"], "transcript_check", "{meta}");
        }
    }
}
