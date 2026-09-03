//! A treatment may be a coordinated set of skills while the CLI-selected
//! skill remains the eval owner and workspace namespace.

use crate::helpers::*;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

mod skill_evidence;

fn add_skill(skill_dir: &Path, name: &str) {
    let root = skill_dir.join(name);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} description\n---\n\n{name} body\n"),
    )
    .unwrap();
}

fn prepare(root: &Path, evals: Value, extra: &[&str]) -> (std::path::PathBuf, std::path::PathBuf) {
    let (skill_dir, cwd) = setup(root, &evals.to_string());
    add_skill(&skill_dir, "supporting-skill");
    add_skill(&skill_dir, "ambient-skill");

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--dry-run"])
        .args(extra)
        .assert()
        .success();

    (skill_dir, cwd)
}

fn multi_evals() -> Value {
    json!({
        "skill_name": ["mr-review", "supporting-skill"],
        "evals": [{
            "id": "e1",
            "prompt": "review this MR",
            "expected_output": "a review"
        }]
    })
}

#[test]
fn mode_a_stages_the_treatment_set_only_in_the_treatment_arm() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (_skill_dir, cwd) = prepare(tmp.path(), multi_evals(), &["--mode", "new-skill"]);

    assert_eq!(
        env_staged_entries(&cwd),
        [
            "ambient-skill",
            "slow-powers-eval-1-with_skill__mr-review",
            "slow-powers-eval-1-with_skill__supporting-skill",
        ]
    );
    assert_eq!(
        staged_entries(&cli_env_dir(&cwd, "g1", "without_skill").join(".claude/skills")),
        ["ambient-skill"]
    );
}

#[test]
fn conditions_and_dispatch_record_the_ordered_treatment_roster() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = prepare(tmp.path(), multi_evals(), &["--mode", "new-skill"]);
    let iteration = iteration_dir(&cwd);

    let conditions = read_json(&iteration.join("conditions.json"));
    let treatment = &conditions["conditions"][0]["skills"];
    assert_eq!(treatment.as_array().unwrap().len(), 2);
    assert_eq!(treatment[0]["name"], "mr-review");
    assert_eq!(treatment[1]["name"], "supporting-skill");
    assert!(
        treatment[0]["skill_path"]
            .as_str()
            .unwrap()
            .contains("/.skills/mr-review/SKILL.md")
    );
    assert!(
        treatment[1]["skill_path"]
            .as_str()
            .unwrap()
            .contains("/.skills/supporting-skill/SKILL.md")
    );
    assert_eq!(conditions["conditions"][1]["skills"], json!([]));

    let dispatch = read_json(&iteration.join("dispatch.json"));
    assert_eq!(
        dispatch["skill_name"],
        json!(["mr-review", "supporting-skill"])
    );
    let task_treatment = dispatch["tasks"][0]["skills"].as_array().unwrap();
    assert_eq!(task_treatment.len(), 2);
    for (task_skill, condition_skill) in task_treatment.iter().zip(treatment.as_array().unwrap()) {
        assert_eq!(task_skill["name"], condition_skill["name"]);
        assert_eq!(task_skill["skill_path"], condition_skill["skill_path"]);
        assert_eq!(
            task_skill["staged_skill_slug"],
            condition_skill["staged_skill_slug"]
        );
        assert!(condition_skill.get("staged_skill_path").is_none());
        let staged_path = task_skill["staged_skill_path"].as_str().unwrap();
        assert!(staged_path.contains("/env-g1-with_skill/.claude/skills/"));
        assert!(staged_path.ends_with("/SKILL.md"));
    }
    assert_eq!(dispatch["tasks"][1]["skills"], json!([]));
    assert_eq!(
        dispatch["tasks"][0]["available_skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|skill| skill["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "ambient-skill",
            // Claude advertises the natural frontmatter name; the roster's
            // staged_skill_slug is the deterministic invocation identifier.
            "mr-review",
            "supporting-skill",
        ]
    );

    for task in dispatch["tasks"].as_array().unwrap() {
        write_default_task_result(&cwd, task, "done");
    }
    skill_eval()
        .current_dir(&cwd)
        .args(["record-runs", "--skill-dir"])
        .arg(skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .success();
    for task in dispatch["tasks"].as_array().unwrap() {
        let run = read_json(Path::new(task["run_record_path"].as_str().unwrap()));
        assert_eq!(run["skills"], task["skills"]);
    }
}

#[test]
fn a_one_member_list_still_uses_list_artifacts_and_indexed_skill_evidence_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = json!({
        "skill_name": ["mr-review"],
        "evals": [{
            "id": "e1",
            "prompt": "review this MR",
            "expected_output": "a review"
        }]
    });
    let (skill_dir, cwd) = prepare(
        tmp.path(),
        evals,
        &["--mode", "new-skill", "--harness", "codex"],
    );
    let iteration = iteration_dir(&cwd);
    let conditions = read_json(&iteration.join("conditions.json"));
    assert_eq!(
        conditions["conditions"][0]["skills"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(conditions["conditions"][1]["skills"], json!([]));
    assert_eq!(
        conditions["skill_source"]["skills"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let dispatch = read_json(&iteration.join("dispatch.json"));
    assert_eq!(dispatch["skill_name"], json!(["mr-review"]));
    for task in dispatch["tasks"].as_array().unwrap() {
        fs::write(
            Path::new(task["run_record_path"].as_str().unwrap()),
            serde_json::to_vec_pretty(&json!({
                "eval_id": task["eval_id"],
                "condition": task["condition"],
                "skill_path": task["skill_path"],
                "skills": task["skills"],
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
            "codex",
            "--iteration",
            "1",
        ])
        .assert()
        .success();
    let tasks = read_json(&iteration.join("judge-tasks.json"));
    assert!(
        tasks["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| task["is_meta"] != true),
        "Codex skill access is graded locally"
    );
    let evidence = read_json(
        &iteration.join("eval-e1/with_skill/judge-responses/__skill_invoked__skill-1.json"),
    );
    assert_eq!(evidence["passed"], false);
    assert_eq!(evidence["grader"], "transcript_check");
    assert!(evidence["evidence"].as_str().unwrap().contains("mr-review"));

    skill_eval()
        .current_dir(&cwd)
        .args(["snapshot", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success();
    assert!(
        cwd.join(".eval-magic/mr-review/snapshots/baseline/skills/mr-review/SKILL.md")
            .exists()
    );
}

#[test]
fn the_cli_selected_eval_owner_must_belong_to_a_multi_skill_treatment() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = json!({
        "skill_name": ["supporting-skill", "ambient-skill"],
        "evals": [{"id": "e1", "prompt": "review", "expected_output": "review"}]
    });
    let (skill_dir, cwd) = setup(tmp.path(), &evals.to_string());
    add_skill(&skill_dir, "supporting-skill");
    add_skill(&skill_dir, "ambient-skill");

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "eval owner 'mr-review' must be listed in skill_name",
        ));
}

#[test]
fn a_multi_skill_treatment_rejects_a_single_stage_name_override() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &multi_evals().to_string());
    add_skill(&skill_dir, "supporting-skill");

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--dry-run",
            "--stage-name",
            "custom",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "--stage-name is only supported for a single skill under test",
        ));
}

#[test]
fn mode_b_snapshots_and_stages_every_treatment_member() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &multi_evals().to_string());
    add_skill(&skill_dir, "supporting-skill");

    skill_eval()
        .current_dir(&cwd)
        .args(["snapshot", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success();

    let snapshot = cwd.join(".eval-magic/mr-review/snapshots/baseline/skills");
    assert!(snapshot.join("mr-review/SKILL.md").exists());
    assert!(snapshot.join("supporting-skill/SKILL.md").exists());

    fs::write(
        skill_dir.join("mr-review/SKILL.md"),
        "---\nname: mr-review\ndescription: owner\n---\n\nNEW OWNER\n",
    )
    .unwrap();
    fs::write(
        skill_dir.join("supporting-skill/SKILL.md"),
        "---\nname: supporting-skill\ndescription: support\n---\n\nNEW SUPPORT\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "revision", "--dry-run"])
        .assert()
        .success();

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(
        conditions["conditions"][0]["skills"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        conditions["conditions"][1]["skills"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let old_root = cli_env_dir(&cwd, "g1", "old_skill").join(".claude/skills");
    let new_root = cli_env_dir(&cwd, "g1", "new_skill").join(".claude/skills");
    assert!(
        read_str(&old_root.join("slow-powers-eval-1-old_skill__supporting-skill/SKILL.md"))
            .contains("supporting-skill body")
    );
    assert!(
        read_str(&new_root.join("slow-powers-eval-1-new_skill__supporting-skill/SKILL.md"))
            .contains("NEW SUPPORT")
    );
}

#[test]
fn a_failed_multi_skill_snapshot_leaves_no_partial_label() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = json!({
        "skill_name": ["mr-review", "missing-skill"],
        "evals": [{"id": "e1", "prompt": "review", "expected_output": "review"}]
    });
    let (skill_dir, cwd) = setup(tmp.path(), &evals.to_string());

    skill_eval()
        .current_dir(&cwd)
        .args(["snapshot", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .failure();

    assert!(
        !cwd.join(".eval-magic/mr-review/snapshots/baseline")
            .exists()
    );
}

#[test]
fn deterministic_grading_reports_each_skill_and_suite_invocation_is_any_member() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (_skill_dir, cwd) = prepare(tmp.path(), multi_evals(), &["--mode", "new-skill"]);
    let iteration = iteration_dir(&cwd);
    let dispatch = read_json(&iteration.join("dispatch.json"));

    for task in dispatch["tasks"].as_array().unwrap() {
        let run_path = Path::new(task["run_record_path"].as_str().unwrap());
        let is_treatment = task["condition"] == "with_skill";
        let invocations = if is_treatment {
            json!([{
                "name": "Skill",
                "args": {"skill": task["skills"][1]["staged_skill_slug"]},
                "ordinal": 0
            }])
        } else {
            json!([])
        };
        fs::write(
            run_path,
            serde_json::to_vec_pretty(&json!({
                "eval_id": task["eval_id"],
                "condition": task["condition"],
                "skill_path": task["skill_path"],
                "skills": task["skills"],
                "prompt": task["user_prompt"],
                "files": task["files"],
                "final_message": "done",
                "tool_invocations": invocations,
                "total_tokens": null,
                "duration_ms": null
            }))
            .unwrap(),
        )
        .unwrap();
    }

    let grade = |finalize: bool| {
        let mut cmd = skill_eval();
        cmd.current_dir(&cwd)
            .args(["grade", "--skill-dir"])
            .arg(tmp.path().join("skill-dir"))
            .args(["--skill", "mr-review", "--iteration", "1"]);
        if finalize {
            cmd.arg("--finalize");
        }
        cmd.assert().success();
    };
    grade(false);

    let responses = iteration.join("eval-e1/with_skill/judge-responses");
    assert_eq!(
        read_json(&responses.join("__skill_invoked__skill-1.json"))["passed"],
        false
    );
    assert_eq!(
        read_json(&responses.join("__skill_invoked__skill-2.json"))["passed"],
        true
    );
    grade(true);

    let grading = read_json(&iteration.join("eval-e1/with_skill/grading.json"));
    assert_eq!(grading["meta_results"].as_array().unwrap().len(), 2);
    assert_eq!(grading["meta_results"][0]["skill_name"], "mr-review");
    assert_eq!(grading["meta_results"][1]["skill_name"], "supporting-skill");
    assert_eq!(grading["meta_summary"]["skill_invoked"], true);

    skill_eval()
        .current_dir(&cwd)
        .args(["aggregate", "--skill-dir"])
        .arg(tmp.path().join("skill-dir"))
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .success();
    let benchmark = read_json(&iteration.join("benchmark.json"));
    let per_skill = &benchmark["run_summary"]["with_skill"]["skill_invocations"];
    assert_eq!(per_skill["mr-review"], json!({"n": 1, "rate": 0.0}));
    assert_eq!(per_skill["supporting-skill"], json!({"n": 1, "rate": 1.0}));
    assert!(
        benchmark["validity_warnings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|warning| !warning.as_str().unwrap().contains("invocation rate")),
        "partial treatment invocation satisfies the suite-level validity check"
    );
}

#[test]
fn provenance_names_the_source_and_revision_of_every_treatment_member() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), &multi_evals().to_string());
    add_skill(&skill_dir, "supporting-skill");
    for args in [
        vec!["init", "--quiet", "--initial-branch", "main", "."],
        vec!["add", "--all"],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&skill_dir)
                .status()
                .unwrap()
                .success()
        );
    }
    assert!(
        std::process::Command::new("git")
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@localhost",
                "commit",
                "--quiet",
                "--no-gpg-sign",
                "-m",
                "test setup",
            ])
            .current_dir(&skill_dir)
            .status()
            .unwrap()
            .success()
    );

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--dry-run"])
        .assert()
        .success();

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    let source = &conditions["skill_source"];
    assert_eq!(source["eval_owner"], "mr-review");
    assert!(
        source.get("siblings").is_none(),
        "there are no ambient siblings"
    );
    let skills = source["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 2);
    assert_eq!(skills[0]["name"], "mr-review");
    assert_eq!(skills[1]["name"], "supporting-skill");
    for skill in skills {
        assert!(
            skill["resolved_path"]
                .as_str()
                .unwrap()
                .ends_with(skill["name"].as_str().unwrap())
        );
        assert_eq!(skill["revision"].as_str().unwrap().len(), 40);
    }
}

#[test]
fn no_stage_inlines_every_treatment_member_without_staging_ambient_skills() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (_skill_dir, cwd) = prepare(
        tmp.path(),
        multi_evals(),
        &["--mode", "new-skill", "--no-stage"],
    );
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let treatment = &dispatch["tasks"][0];
    let prompt = read_str(Path::new(
        treatment["dispatch_prompt_path"].as_str().unwrap(),
    ));

    assert!(prompt.contains("<skill name=\"mr-review\">"));
    assert!(prompt.contains("<skill name=\"supporting-skill\">"));
    assert!(prompt.contains("supporting-skill body"));
    assert!(
        treatment["skills"]
            .as_array()
            .unwrap()
            .iter()
            .all(|skill| skill["staged_skill_slug"].is_null())
    );
    assert!(
        !cli_env_dir(&cwd, "g1", "with_skill")
            .join(".claude/skills")
            .exists()
    );
}

#[test]
fn live_source_detection_checks_every_treatment_member() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = prepare(tmp.path(), multi_evals(), &["--mode", "new-skill"]);
    let iteration = iteration_dir(&cwd);
    let dispatch = read_json(&iteration.join("dispatch.json"));
    let task = &dispatch["tasks"][0];
    fs::write(
        Path::new(task["run_record_path"].as_str().unwrap()),
        serde_json::to_vec_pretty(&json!({
            "eval_id": task["eval_id"],
            "condition": task["condition"],
            "skill_path": task["skill_path"],
            "skills": task["skills"],
            "prompt": task["user_prompt"],
            "files": task["files"],
            "final_message": "done",
            "tool_invocations": [{
                "name": "Read",
                "args": {
                    "file_path": wire_path(&skill_dir.join("supporting-skill/SKILL.md"))
                },
                "ordinal": 0
            }],
            "total_tokens": null,
            "duration_ms": null
        }))
        .unwrap(),
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["detect-stray-writes", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .success();

    let report = read_json(&iteration.join("stray-writes.json"));
    assert_eq!(report["totals"]["live_source_reads"], 1);
    assert!(
        report["runs"][0]["live_source_reads"][0]["path"]
            .as_str()
            .unwrap()
            .contains("supporting-skill/SKILL.md")
    );
}
