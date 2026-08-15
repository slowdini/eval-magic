//! The `detect-stray-writes` subcommand.

use crate::helpers::{resolved, skill_eval};
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use tempfile::TempDir;

/// `detect-stray-writes` reports a live-source read per run in stray-writes.json.
#[test]
fn detect_stray_writes_reports_live_source_reads() {
    use serde_json::json;

    let tmp = TempDir::new().unwrap();
    // realpath: the binary reads its cwd resolved (macOS /var → /private/var), so
    // fixture paths must match that form for prefix checks to line up.
    let root = resolved(tmp.path());
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
        .join(".eval-magic")
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

/// With no transcript tool-calls to inspect, `detect-stray-writes` must not
/// report a clean pass: an empty `tool_invocations` array means it had nothing
/// to check, so it flags the result unverifiable instead of falsely confident.
#[test]
fn detect_stray_writes_flags_unverifiable_when_nothing_was_inspected() {
    use serde_json::json;

    let tmp = TempDir::new().unwrap();
    let root = resolved(tmp.path());
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
        .join(".eval-magic")
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

    // A recorded run whose transcript never linked: final message present,
    // tool_invocations empty.
    fs::write(
        cond_dir.join("run.json"),
        serde_json::to_string(&json!({
            "eval_id": "e1",
            "condition": "old_skill",
            "skill_path": skill_md,
            "prompt": "do the task",
            "files": [],
            "final_message": "done",
            "tool_invocations": [],
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
        .success()
        .stderr(contains("Unverifiable"))
        .stdout(contains("No out-of-bounds").not());
}

/// Without a `dispatch.json` eval_root for the run, the detector must not
/// fabricate a task boundary. It skips out-of-bounds classification and logs why.
#[test]
fn detect_stray_writes_skips_write_classification_without_dispatch_eval_root() {
    use serde_json::json;

    let tmp = TempDir::new().unwrap();
    let root = resolved(tmp.path());
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
        .join(".eval-magic")
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

    // The agent wrote into the isolated env's outputs tree — the real new-layout
    // location, which is NOT under the old `<cond_dir>/outputs` fallback path.
    let env_output = iteration_dir
        .join("env")
        .join(".eval-magic-outputs")
        .join("eval-e1")
        .join("old_skill")
        .join("answer.md")
        .to_string_lossy()
        .into_owned();

    // No dispatch.json is written: the run has no recorded eval_root.
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
                {"name": "Write", "args": {"file_path": env_output}, "ordinal": 0},
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
        .success()
        .stderr(contains("no eval_root in dispatch.json"));

    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("stray-writes.json")).unwrap())
            .unwrap();
    // The env-layout write is NOT mis-flagged: with no known boundary the detector
    // refuses to guess rather than fabricating a wrong one.
    assert_eq!(report["totals"]["violations"], json!(0));
}

/// With `dispatch.json` carrying the task's eval_root, the detector permits
/// ordinary source edits anywhere in that private task environment and flags
/// only writes that leave it.
#[test]
fn detect_stray_writes_uses_eval_root_boundary_from_dispatch() {
    use serde_json::json;

    let tmp = TempDir::new().unwrap();
    let root = resolved(tmp.path());
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
        .join(".eval-magic")
        .join("mr-review")
        .join("iteration-1");
    let cond_dir = iteration_dir.join("eval-e1").join("old_skill");
    fs::create_dir_all(&cond_dir).unwrap();

    let eval_root = iteration_dir.join("env-g1-old_skill").join("eval-e1");
    let outputs_dir = eval_root
        .join(".eval-magic-outputs")
        .join("eval-e1")
        .join("old_skill");
    let output_artifact = outputs_dir.join("answer.md").to_string_lossy().into_owned();
    let source_edit = eval_root.join("src/lib.rs").to_string_lossy().into_owned();
    let stray = iteration_dir
        .join("notes.md")
        .to_string_lossy()
        .into_owned();

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

    // dispatch.json carries both framework outputs and the broader task boundary.
    fs::write(
        iteration_dir.join("dispatch.json"),
        serde_json::to_string(&json!({
            "tasks": [
                {
                    "eval_id": "e1",
                    "condition": "old_skill",
                    "outputs_dir": outputs_dir.to_string_lossy(),
                    "eval_root": eval_root.to_string_lossy(),
                }
            ],
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
                {"name": "Write", "args": {"file_path": output_artifact}, "ordinal": 0},
                {"name": "Edit", "args": {"file_path": source_edit}, "ordinal": 1},
                {"name": "Write", "args": {"file_path": stray}, "ordinal": 2},
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
    assert_eq!(report["totals"]["violations"], json!(1));
    assert_eq!(report["runs"].as_array().unwrap().len(), 1);
    assert_eq!(report["runs"][0]["violations"][0]["path"], json!(stray));
}

/// `detect-stray-writes` scans every `run-<k>` subdirectory of a condition cell
/// and tags each report entry with its run index.
#[test]
fn detect_stray_writes_scans_nested_run_dirs_and_reports_run_index() {
    use serde_json::json;

    let tmp = TempDir::new().unwrap();
    let root = resolved(tmp.path());
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
        .join(".eval-magic")
        .join("mr-review")
        .join("iteration-1");
    let cond_dir = iteration_dir.join("eval-e1").join("old_skill");
    fs::create_dir_all(&iteration_dir).unwrap();

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

    // run-1 is clean; run-2 reads the live skill source.
    for (k, invocations) in [
        (1, json!([])),
        (
            2,
            json!([{"name": "Read", "args": {"file_path": skill_md}, "ordinal": 0}]),
        ),
    ] {
        let run_dir = cond_dir.join(format!("run-{k}"));
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(
            run_dir.join("run.json"),
            serde_json::to_string(&json!({
                "eval_id": "e1",
                "condition": "old_skill",
                "skill_path": skill_md,
                "prompt": "do the task",
                "files": [],
                "final_message": "done",
                "tool_invocations": invocations,
                "total_tokens": null,
                "duration_ms": null,
            }))
            .unwrap(),
        )
        .unwrap();
    }

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
    let runs = report["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "only the offending run is reported");
    assert_eq!(runs[0]["eval_id"], json!("e1"));
    assert_eq!(runs[0]["condition"], json!("old_skill"));
    assert_eq!(runs[0]["run_index"], json!(2));
}
