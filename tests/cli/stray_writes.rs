//! The `detect-stray-writes` subcommand.

use crate::helpers::skill_eval;
use std::fs;
use tempfile::TempDir;

/// `detect-stray-writes` reports a live-source read per run in stray-writes.json.
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
