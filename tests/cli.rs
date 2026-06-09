//! Integration tests for the CLI surface, driving the built `skill-eval`
//! binary. Mirrors the subprocess-style integration tests in eval-runner
//! (`cli.test.ts`). These pin the command tree and dispatch behavior of the
//! Phase-0 scaffold; per-command behavior is tested as each module is ported.

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;
use std::fs;
use tempfile::TempDir;

fn skill_eval() -> Command {
    Command::cargo_bin("skill-eval").expect("binary `skill-eval` should build")
}

/// A minimal valid `evals.json` body.
const VALID_EVALS: &str = r#"{ "skill_name": "demo", "evals": [
    { "id": "e1", "prompt": "p", "expected_output": "o" } ] }"#;

/// Build `<root>/<skill>/evals/evals.json` with the given contents.
fn write_evals(root: &std::path::Path, skill: &str, contents: &str) {
    let dir = root.join(skill).join("evals");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("evals.json"), contents).unwrap();
}

/// `--help` succeeds and lists the subcommands ported from eval-runner.
#[test]
fn help_lists_subcommands() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("record-runs"))
        .stdout(contains("grade"))
        .stdout(contains("validate"))
        .stdout(contains("aggregate"));
}

/// The binary name in help output is the published command name.
#[test]
fn help_uses_published_binary_name() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("skill-eval"));
}

/// A recognized subcommand whose module hasn't been ported yet dispatches to a
/// handler that reports "not yet implemented" and exits non-zero.
#[test]
fn unported_subcommand_reports_not_yet_implemented() {
    skill_eval()
        .arg("snapshot")
        .assert()
        .failure()
        .stderr(contains("not yet implemented"));
}

/// `validate` over a dir of valid evals succeeds and prints a ✓ per file.
#[test]
fn validate_succeeds_on_valid_evals() {
    let tmp = TempDir::new().unwrap();
    write_evals(tmp.path(), "good", VALID_EVALS);

    skill_eval()
        .arg("validate")
        .arg("--skill-dir")
        .arg(tmp.path())
        .assert()
        .success()
        .stdout(contains("✓ good/evals/evals.json"))
        .stdout(contains("Validated 1 evals.json file(s); 0 failed."));
}

/// `validate` exits non-zero and prints a ✗ when a file fails validation.
#[test]
fn validate_fails_on_invalid_evals() {
    let tmp = TempDir::new().unwrap();
    write_evals(tmp.path(), "bad", r#"{ "skill_name": "x", "evals": [] }"#);

    skill_eval()
        .arg("validate")
        .arg("--skill-dir")
        .arg(tmp.path())
        .assert()
        .failure()
        .stderr(contains("✗"));
}

/// `validate` without the required `--skill-dir` flag fails with our message.
#[test]
fn validate_requires_skill_dir() {
    skill_eval()
        .arg("validate")
        .assert()
        .failure()
        .stderr(contains("missing required flag --skill-dir"));
}

/// An unknown subcommand is rejected by the parser (clap), not silently
/// accepted.
#[test]
fn unknown_subcommand_is_rejected() {
    skill_eval().arg("does-not-exist").assert().failure();
}

/// The internal `guard` hook entry point is hidden from `--help` (its unique
/// description never appears) yet remains callable.
#[test]
fn guard_subcommand_is_hidden_but_callable() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("PreToolUse hook entry point").not());

    skill_eval().arg("guard").arg("--help").assert().success();
}

/// Write an armed guard marker scoping writes to `<allowed>`, and return its path.
fn write_armed_marker(root: &std::path::Path, allowed: &std::path::Path) -> std::path::PathBuf {
    let skills = root.join(".claude").join("skills");
    fs::create_dir_all(&skills).unwrap();
    let marker = skills.join(".slow-powers-eval-guard.json");
    fs::write(
        &marker,
        format!(
            r#"{{ "active": true, "allowedRoots": ["{}"], "expiresAt": "2999-01-01T00:00:00.000Z" }}"#,
            allowed.display()
        ),
    )
    .unwrap();
    marker
}

/// `guard` denies a Write outside the sandbox: it prints a PreToolUse deny verdict
/// on stdout and still exits 0 (the hook must never fail the session).
#[test]
fn guard_denies_out_of_bounds_write() {
    let tmp = TempDir::new().unwrap();
    let marker = write_armed_marker(tmp.path(), &tmp.path().join("skills-workspace"));

    skill_eval()
        .arg("guard")
        .arg(&marker)
        .write_stdin(r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#)
        .assert()
        .success()
        .stdout(contains(r#""permissionDecision":"deny""#));
}

/// `guard` allows an in-bounds write: empty stdout, exit 0.
#[test]
fn guard_allows_in_bounds_write() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("skills-workspace");
    let marker = write_armed_marker(tmp.path(), &workspace);

    skill_eval()
        .arg("guard")
        .arg(&marker)
        .write_stdin(format!(
            r#"{{ "tool_name": "Write", "tool_input": {{ "file_path": "{}/out.md" }} }}"#,
            workspace.display()
        ))
        .assert()
        .success()
        .stdout("");
}

/// `guard` fails open when the marker is absent: empty stdout, exit 0.
#[test]
fn guard_fails_open_without_marker() {
    let tmp = TempDir::new().unwrap();
    skill_eval()
        .arg("guard")
        .arg(tmp.path().join("nope.json"))
        .write_stdin(r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#)
        .assert()
        .success()
        .stdout("");
}

/// `teardown-guard` reports when no guard is installed (cwd has no marker).
#[test]
fn teardown_guard_reports_nothing_to_remove() {
    let tmp = TempDir::new().unwrap();
    skill_eval()
        .arg("teardown-guard")
        .current_dir(tmp.path())
        .assert()
        .success()
        .stdout(contains("nothing to remove"));
}

/// `teardown-guard` sweeps a stray marker in the cwd and reports removal.
#[test]
fn teardown_guard_removes_installed_guard() {
    let tmp = TempDir::new().unwrap();
    write_armed_marker(tmp.path(), &tmp.path().join("skills-workspace"));

    skill_eval()
        .arg("teardown-guard")
        .current_dir(tmp.path())
        .assert()
        .success()
        .stdout(contains("Write guard removed"));

    assert!(
        !tmp.path()
            .join(".claude/skills/.slow-powers-eval-guard.json")
            .exists()
    );
}

/// `detect-stray-writes` reports a live-source read per run in stray-writes.json.
/// Ports the subprocess CLI test in `detect-stray-writes.test.ts`.
#[test]
fn detect_stray_writes_reports_live_source_reads() {
    use serde_json::json;

    let tmp = TempDir::new().unwrap();
    // realpath: the binary reads its cwd resolved (macOS /var → /private/var), so
    // fixture paths must match that form for prefix checks to line up.
    let root = fs::canonicalize(tmp.path()).unwrap();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    fs::create_dir_all(&skill_sub).unwrap();
    fs::write(
        skill_sub.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
    )
    .unwrap();
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    let cond_dir = iteration_dir.join("eval-e1").join("old_skill");
    fs::create_dir_all(&cond_dir).unwrap();

    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "revision",
            "conditions": [
                {"name": "old_skill", "skill_path": skill_md},
                {"name": "new_skill", "skill_path": skill_md},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();

    fs::write(
        cond_dir.join("run.json"),
        serde_json::to_string(&json!({
            "eval_id": "e1",
            "condition": "old_skill",
            "skill_path": skill_md,
            "prompt": "do the task",
            "files": [],
            "final_message": "done",
            "tool_invocations": [
                {"name": "Read", "args": {"file_path": skill_md}, "ordinal": 0},
                {"name": "Write", "args": {"file_path": cond_dir.join("outputs").join("answer.md").to_string_lossy()}, "ordinal": 1},
            ],
            "total_tokens": null,
            "duration_ms": null,
        }))
        .unwrap(),
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .arg("detect-stray-writes")
        .arg("--skill-dir")
        .arg(&skill_dir)
        .arg("--skill")
        .arg("mr-review")
        .arg("--iteration")
        .arg("1")
        .assert()
        .success();

    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("stray-writes.json")).unwrap())
            .unwrap();
    assert_eq!(report["totals"]["live_source_reads"], json!(1));
    assert_eq!(report["totals"]["violations"], json!(0));
    assert_eq!(report["runs"].as_array().unwrap().len(), 1);
    assert_eq!(report["runs"][0]["eval_id"], json!("e1"));
    assert_eq!(report["runs"][0]["condition"], json!("old_skill"));
    assert_eq!(
        report["runs"][0]["live_source_reads"][0]["tool"],
        json!("Read")
    );
    assert_eq!(
        report["runs"][0]["live_source_reads"][0]["path"],
        json!(skill_md)
    );
}

// --- grade ---

/// A canonicalized temp root (resolves macOS /var → /private/var so the binary's
/// cwd-derived workspace path matches the fixtures it reads).
fn canonical_root() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    (tmp, root)
}

/// Write `<skill_sub>/SKILL.md` and `<skill_sub>/evals/evals.json`.
fn write_skill(skill_sub: &std::path::Path, skill_md: &str, evals: &serde_json::Value) {
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    fs::write(skill_sub.join("SKILL.md"), skill_md).unwrap();
    fs::write(
        skill_sub.join("evals").join("evals.json"),
        serde_json::to_string_pretty(evals).unwrap(),
    )
    .unwrap();
}

/// Run `grade` (optionally `--finalize`) for skill `mr-review`, iteration 1.
fn grade_cmd(cwd: &std::path::Path, skill_dir: &std::path::Path, harness: Option<&str>) -> Command {
    let mut cmd = skill_eval();
    cmd.current_dir(cwd)
        .arg("grade")
        .arg("--skill-dir")
        .arg(skill_dir)
        .arg("--skill")
        .arg("mr-review")
        .arg("--iteration")
        .arg("1");
    if let Some(h) = harness {
        cmd.arg("--harness").arg(h);
    }
    cmd
}

/// `grade` (emit): a Codex staged run routes the skill-invocation meta-check to an
/// LLM judge task whose prompt embeds the SKILL.md content (no code-check).
#[test]
fn grade_codex_staged_run_uses_llm_meta_check_with_skill_content() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nUse the MERGE-RISK-LADDER before writing the final review.",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Review this MR.", "expected_output": "Agent reviews systematically."}
        ]}),
    );
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    let cond_dir = iteration_dir.join("eval-pos-eval").join("with_skill");
    fs::create_dir_all(&cond_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [{"name": "with_skill", "skill_path": skill_md, "staged_skill_slug": "slow-powers-eval-1-with_skill__mr-review"}],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "codex",
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        cond_dir.join("run.json"),
        serde_json::to_string(&json!({
            "eval_id": "pos-eval", "condition": "with_skill", "skill_path": skill_md,
            "prompt": "p", "files": [], "final_message": "I reviewed the MR.",
            "tool_invocations": [{"name": "command_execution", "args": {"command": "ls"}, "ordinal": 0}],
            "total_tokens": 100, "duration_ms": 1000,
        }))
        .unwrap(),
    )
    .unwrap();

    grade_cmd(&cwd, &skill_dir, Some("codex"))
        .assert()
        .success();

    assert!(
        !cond_dir
            .join("judge-responses")
            .join("__skill_invoked.json")
            .exists()
    );
    let tasks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("judge-tasks.json")).unwrap())
            .unwrap();
    let has_meta = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["assertion_id"] == json!("__skill_invoked") && t["is_meta"] == json!(true));
    assert!(has_meta, "expected a meta judge task");
    let prompt =
        fs::read_to_string(cond_dir.join("judge-prompts").join("__skill_invoked.txt")).unwrap();
    assert!(prompt.contains("MERGE-RISK-LADDER"));
}

/// `grade` (emit): evals marked `skill_should_trigger: false` get no meta-check.
#[test]
fn grade_omits_meta_check_for_negative_evals() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Fix the failing build.", "expected_output": "debugs",
             "assertions": [{"id": "a1", "type": "llm_judge", "rubric": "Did it debug?"}]},
            {"id": "neg-eval", "prompt": "Add a --verbose flag.", "expected_output": "feature",
             "skill_should_trigger": false,
             "assertions": [{"id": "a2", "type": "llm_judge", "rubric": "Did it avoid debugging?"}]}
        ]}),
    );
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    fs::create_dir_all(&iteration_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [
                {"name": "with_skill", "skill_path": skill_md},
                {"name": "without_skill", "skill_path": null},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    for eval_id in ["pos-eval", "neg-eval"] {
        for cond in ["with_skill", "without_skill"] {
            let cond_dir = iteration_dir.join(format!("eval-{eval_id}")).join(cond);
            fs::create_dir_all(&cond_dir).unwrap();
            let skill_path = if cond == "with_skill" {
                json!(skill_md)
            } else {
                json!(null)
            };
            fs::write(
                cond_dir.join("run.json"),
                serde_json::to_string(&json!({
                    "eval_id": eval_id, "condition": cond, "skill_path": skill_path,
                    "prompt": "p", "files": [], "final_message": "done",
                    "tool_invocations": [], "total_tokens": 100, "duration_ms": 1000,
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }

    grade_cmd(&cwd, &skill_dir, None).assert().success();

    let tasks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("judge-tasks.json")).unwrap())
            .unwrap();
    let meta_eval_ids: Vec<&str> = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["is_meta"] == json!(true))
        .map(|t| t["eval_id"].as_str().unwrap())
        .collect();
    assert_eq!(meta_eval_ids, vec!["pos-eval"]);
}

/// `grade` (emit): a malformed run.json fails fast with a run-record schema error.
#[test]
fn grade_fails_fast_on_malformed_run_record() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Fix it.", "expected_output": "debugs",
             "assertions": [{"id": "a1", "type": "llm_judge", "rubric": "Did it debug?"}]}
        ]}),
    );

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    fs::create_dir_all(&iteration_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [
                {"name": "with_skill", "skill_path": skill_sub.join("SKILL.md").to_string_lossy()},
                {"name": "without_skill", "skill_path": null},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    for cond in ["with_skill", "without_skill"] {
        let cond_dir = iteration_dir.join("eval-pos-eval").join(cond);
        fs::create_dir_all(&cond_dir).unwrap();
        // Missing required `final_message` and `files` — must be rejected.
        fs::write(
            cond_dir.join("run.json"),
            serde_json::to_string(&json!({
                "eval_id": "pos-eval", "condition": cond, "skill_path": null,
                "prompt": "p", "tool_invocations": [],
            }))
            .unwrap(),
        )
        .unwrap();
    }

    grade_cmd(&cwd, &skill_dir, None)
        .assert()
        .failure()
        .stderr(contains("run-record schema"));
}

/// `grade` (emit): each prompt is written to a file and the inline prompt is
/// dropped from judge-tasks.json (the orchestrator reads it from the file).
#[test]
fn grade_writes_prompt_files_and_drops_inline_prompt() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Fix it.", "expected_output": "debugs",
             "assertions": [{"id": "a1", "type": "llm_judge", "rubric": "Did it debug?"}]}
        ]}),
    );
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    fs::create_dir_all(&iteration_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [
                {"name": "with_skill", "skill_path": skill_md},
                {"name": "without_skill", "skill_path": null},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    for cond in ["with_skill", "without_skill"] {
        let cond_dir = iteration_dir.join("eval-pos-eval").join(cond);
        fs::create_dir_all(&cond_dir).unwrap();
        let skill_path = if cond == "with_skill" {
            json!(skill_md)
        } else {
            json!(null)
        };
        fs::write(
            cond_dir.join("run.json"),
            serde_json::to_string(&json!({
                "eval_id": "pos-eval", "condition": cond, "skill_path": skill_path,
                "prompt": "p", "files": [], "final_message": "done",
                "tool_invocations": [], "total_tokens": 100, "duration_ms": 1000,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    grade_cmd(&cwd, &skill_dir, None).assert().success();

    let tasks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("judge-tasks.json")).unwrap())
            .unwrap();
    let tasks = tasks["tasks"].as_array().unwrap();
    assert!(!tasks.is_empty());
    for t in tasks {
        assert!(
            t.get("dispatch_prompt").is_none(),
            "inline prompt not stripped"
        );
        let prompt_path = t["dispatch_prompt_path"].as_str().unwrap();
        let assertion_id = t["assertion_id"].as_str().unwrap();
        assert!(prompt_path.ends_with(&format!("{assertion_id}.txt")));
        let contents = fs::read_to_string(prompt_path).unwrap();
        assert!(contents.contains(t["response_path"].as_str().unwrap()));
    }
}

/// `grade --finalize`: folds a code-checked meta result + an llm_judge response
/// into a schema-valid grading.json with the right summaries.
#[test]
fn grade_finalize_folds_responses_into_grading() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    write_skill(
        &skill_sub,
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
        &json!({"skill_name": "mr-review", "evals": [
            {"id": "pos-eval", "prompt": "Fix it.", "expected_output": "debugs",
             "assertions": [{"id": "a1", "type": "llm_judge", "rubric": "Did it debug?"}]}
        ]}),
    );
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();
    let slug = "slow-powers-eval-1-with_skill__mr-review";

    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    let cond_dir = iteration_dir.join("eval-pos-eval").join("with_skill");
    fs::create_dir_all(&cond_dir).unwrap();
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "new-skill",
            "conditions": [{"name": "with_skill", "skill_path": skill_md, "staged_skill_slug": slug}],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    // Transcript invokes the staged skill → meta is code-checked to passed.
    fs::write(
        cond_dir.join("run.json"),
        serde_json::to_string(&json!({
            "eval_id": "pos-eval", "condition": "with_skill", "skill_path": skill_md,
            "prompt": "p", "files": [], "final_message": "done",
            "tool_invocations": [{"name": "Skill", "args": {"skill": slug}, "ordinal": 0}],
            "total_tokens": 100, "duration_ms": 1000,
        }))
        .unwrap(),
    )
    .unwrap();

    // Emit (writes the code-checked meta response + the a1 judge task).
    grade_cmd(&cwd, &skill_dir, None).assert().success();
    // The orchestrator's a1 verdict.
    fs::write(
        cond_dir.join("judge-responses").join("a1.json"),
        serde_json::to_string(
            &json!({"passed": true, "evidence": "it debugged", "confidence": 0.9}),
        )
        .unwrap(),
    )
    .unwrap();

    // Finalize.
    grade_cmd(&cwd, &skill_dir, None)
        .arg("--finalize")
        .assert()
        .success();

    let grading: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(cond_dir.join("grading.json")).unwrap()).unwrap();
    assert_eq!(grading["summary"]["passed"], json!(1));
    assert_eq!(grading["summary"]["total"], json!(1));
    assert_eq!(grading["summary"]["pass_rate"], json!(1.0));
    assert_eq!(grading["assertion_results"][0]["id"], json!("a1"));
    assert_eq!(grading["assertion_results"][0]["passed"], json!(true));
    assert_eq!(grading["meta_summary"]["skill_invoked"], json!(true));
}
