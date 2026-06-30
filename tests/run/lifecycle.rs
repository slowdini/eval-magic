//! Guard install/teardown, workspace reclamation, run-nonce namespacing,
//! bootstrap framing, and `--only` filtering.

use crate::helpers::*;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use std::path::Path;

#[test]
fn guard_installs_pretooluse_hook_under_env() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // The guard arms inside each per-(group, condition) env — the agent-under-test's cwd.
    let settings = cli_env_dir(&cwd, "g1", "with_skill").join(".claude/settings.local.json");

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();
    assert!(settings.exists());
    let parsed = read_json(&settings);
    assert!(
        parsed["hooks"]["PreToolUse"][0]["matcher"]
            .as_str()
            .unwrap()
            .contains("Write")
    );
    // Nothing is armed at the invocation cwd anymore.
    assert!(!cwd.join(".claude/settings.local.json").exists());

    // `teardown-guard` operates at the invocation cwd, so it does not reach the
    // env-scoped guard: this is a transitional no-op, reconciled when the loop runs
    // inside the env session / teardown is reworked. The env is disposable
    // and the guard auto-expires (6h TTL); full `teardown` reclaims it (see
    // `teardown_reclaims_workspace_and_env_guard`).
    skill_eval()
        .current_dir(&cwd)
        .args(["teardown-guard", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success();
    assert!(settings.exists(), "env guard survives a cwd teardown-guard");
}

#[test]
fn finalize_warns_about_armed_per_env_guard_for_default_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // The bare default run is hybrid: `--guard` arms a marker in each per-(group,
    // condition) env. `finalize` runs from the invocation cwd, not inside any env, but
    // the reworked finalize walks the per-env markers, so it reminds the operator the
    // guard is still armed. (finalize only warns; `teardown` disarms — the marker
    // survives finalize.)
    let marker =
        cli_env_dir(&cwd, "g1", "with_skill").join(".claude/skills/.slow-powers-eval-guard.json");

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();
    assert!(marker.exists());

    skill_eval()
        .current_dir(&cwd)
        .args(["finalize", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .success()
        .stdout(contains("Guard still armed"));

    assert!(marker.exists());
}

#[test]
fn finalize_does_not_warn_when_guard_is_not_armed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill"])
        .assert()
        .success();

    skill_eval()
        .current_dir(&cwd)
        .args(["finalize", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--iteration", "1"])
        .assert()
        .success()
        .stdout(contains("Finalize complete"))
        .stdout(contains("Guard still armed").not());
}

#[test]
fn teardown_reclaims_workspace_and_env_guard() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let settings = cli_env_dir(&cwd, "g1", "with_skill").join(".claude/settings.local.json");
    let staged = cli_env_dir(&cwd, "g1", "with_skill").join(".claude/skills");

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();
    assert!(settings.exists());
    assert!(staged.exists());

    // Full `teardown` reclaims the workspace iteration; the env (and its guard) lives
    // inside it, so removing the workspace removes the env guard too — this is what makes
    // deferring the cwd teardown-guard rework safe.
    skill_eval()
        .current_dir(&cwd)
        .args(["teardown", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success();
    assert!(!cwd.join(".eval-magic").exists());
    assert!(!settings.exists());
    assert!(!staged.exists());
    assert!(!cwd.join(".claude").exists());
}

#[test]
fn teardown_preserves_iteration_with_uncommitted_results() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill"])
        .assert()
        .success();

    // Simulate a graded-but-not-promoted run.
    fs::write(
        iteration_dir(&cwd).join("benchmark.json"),
        "{\"delta\":{\"pass_rate\":0.4}}\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["teardown", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .success()
        .stderr(contains("iteration-1"))
        .stderr(contains("promote-baseline"));

    assert!(iteration_dir(&cwd).exists());
}

#[test]
fn normal_run_does_not_install_guard() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();
    assert!(!cwd.join(".claude/settings.local.json").exists());
}

#[test]
fn namespaces_agent_description_and_records_run_nonce() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let nonce = dispatch["run_nonce"].as_str().unwrap();
    assert!(!nonce.is_empty());
    for task in dispatch["tasks"].as_array().unwrap() {
        let condition = task["condition"].as_str().unwrap();
        let desc = task["agent_description"].as_str().unwrap();
        assert!(desc.ends_with(&format!(":{condition}:i1-{nonce}")));
    }
    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(conditions["run_nonce"].as_str().unwrap(), nonce);
}

#[test]
fn records_operator_declared_models_and_label_in_manifests() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .args(["--agent-model", "claude-haiku-4-5-20251001"])
        .args(["--judge-model", "claude-opus-4-8"])
        .args(["--label", "canonical-run"])
        .assert()
        .success();

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(
        conditions["agent_model"].as_str().unwrap(),
        "claude-haiku-4-5-20251001"
    );
    assert_eq!(
        conditions["judge_model"].as_str().unwrap(),
        "claude-opus-4-8"
    );
    assert_eq!(conditions["label"].as_str().unwrap(), "canonical-run");

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(
        dispatch["agent_model"].as_str().unwrap(),
        "claude-haiku-4-5-20251001"
    );
    assert_eq!(dispatch["judge_model"].as_str().unwrap(), "claude-opus-4-8");
    assert_eq!(dispatch["label"].as_str().unwrap(), "canonical-run");
}

#[test]
fn omitted_models_and_label_are_absent_from_conditions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert!(conditions.get("agent_model").is_none());
    assert!(conditions.get("judge_model").is_none());
    assert!(conditions.get("label").is_none());
}

#[test]
fn bootstrap_content_prepended_before_available_skills() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    let bootstrap = cwd.join("my-bootstrap.md");
    fs::write(&bootstrap, "MY CUSTOM EVAL FRAMING").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--bootstrap"])
        .arg(&bootstrap)
        .arg("--dry-run")
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["condition"] == "with_skill")
        .unwrap();
    let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
    let boot_idx = prompt.find("MY CUSTOM EVAL FRAMING").unwrap();
    let list_idx = prompt
        .find("The following skills are available for use with the Skill tool:")
        .unwrap();
    assert!(list_idx > boot_idx);
}

#[test]
fn runs_flag_expands_dispatches_into_run_dirs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review MR 1", "expected_output": "a review" },
        { "id": "e2", "prompt": "review MR 2", "expected_output": "a review" } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--runs",
            "2",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(contains(
            "8 dispatches required (2 evals × 2 conditions × 2 runs)",
        ));

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert_eq!(dispatch["runs"], serde_json::json!(2));
    let tasks = dispatch["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 8);

    let mut descriptions = std::collections::HashSet::new();
    for task in tasks {
        let k = task["run_index"].as_u64().unwrap();
        assert!(k == 1 || k == 2);
        let run_seg = format!("/run-{k}/");
        assert!(
            task["run_record_path"].as_str().unwrap().contains(&run_seg),
            "run.json not under its run dir: {}",
            task["run_record_path"]
        );
        // Outputs live inside the env, namespaced per run so concurrent
        // same-batch subagents can't collide; run-<k> is the leaf segment.
        let outputs_dir = task["outputs_dir"].as_str().unwrap();
        assert!(
            outputs_dir.contains(".eval-magic-outputs/")
                && outputs_dir.ends_with(&format!("run-{k}")),
            "outputs not namespaced under env per run: {outputs_dir}"
        );
        let desc = task["agent_description"].as_str().unwrap();
        assert!(
            desc.contains(&format!(":r{k}:")),
            "missing run segment in description: {desc}"
        );
        assert!(descriptions.insert(desc.to_string()), "duplicate: {desc}");
    }
    for eval in ["e1", "e2"] {
        for cond in ["with_skill", "without_skill"] {
            for k in [1, 2] {
                // Meta run dir (run.json / timing.json) above the env.
                let run_dir = iteration_dir(&cwd)
                    .join(format!("eval-{eval}"))
                    .join(cond)
                    .join(format!("run-{k}"));
                assert!(run_dir.is_dir(), "missing meta run dir {run_dir:?}");
                // Per-run outputs dir inside the condition's env.
                let out_dir = cli_env_dir(&cwd, "g1", cond)
                    .join(".eval-magic-outputs")
                    .join(format!("eval-{eval}"))
                    .join(cond)
                    .join(format!("run-{k}"));
                assert!(out_dir.is_dir(), "missing env outputs dir {out_dir:?}");
            }
        }
    }
}

#[test]
fn runs_one_keeps_flat_single_run_layout() {
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
            "--runs",
            "1",
            "--dry-run",
        ])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        assert!(task.get("run_index").is_none(), "run_index on single run");
        assert!(!task["run_record_path"].as_str().unwrap().contains("/run-"));
    }
    // Flat single-run layout: the meta cond dir exists, with no run-1/ nesting.
    let cond_dir = iteration_dir(&cwd).join("eval-e1").join("with_skill");
    assert!(cond_dir.is_dir());
    assert!(!cond_dir.join("run-1").exists());
    // Outputs live inside the condition's env, flat (no run-1/ segment) for a
    // single-run cell.
    let out_dir =
        cli_env_dir(&cwd, "g1", "with_skill").join(".eval-magic-outputs/eval-e1/with_skill");
    assert!(out_dir.is_dir());
    assert!(!out_dir.join("run-1").exists());
}

#[test]
fn runs_zero_is_rejected() {
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
            "--runs",
            "0",
            "--dry-run",
        ])
        .assert()
        .failure();
}

#[test]
fn per_eval_runs_overrides_the_flag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review MR 1", "expected_output": "a review", "runs": 3 },
        { "id": "e2", "prompt": "review MR 2", "expected_output": "a review" } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let tasks = dispatch["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 8, "3 runs × 2 conds for e1 + 1 run × 2 for e2");
    let e1_indices: Vec<u64> = tasks
        .iter()
        .filter(|t| t["eval_id"] == "e1" && t["condition"] == "with_skill")
        .map(|t| t["run_index"].as_u64().unwrap())
        .collect();
    assert_eq!(e1_indices, vec![1, 2, 3]);
    for task in tasks.iter().filter(|t| t["eval_id"] == "e2") {
        assert!(task.get("run_index").is_none());
        assert!(!task["run_record_path"].as_str().unwrap().contains("/run-"));
    }
}

#[test]
fn only_restricts_dispatches_to_named_ids() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review MR 1", "expected_output": "a review" },
        { "id": "e2", "prompt": "review MR 2", "expected_output": "a review" } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--only",
            "e1",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(contains("1 evals × 2 conditions"));

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let ids: Vec<&str> = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["eval_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["e1", "e1"]);
}

#[test]
fn only_with_unknown_id_exits_nonzero() {
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
            "--only",
            "nope",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(contains("unknown eval id(s): nope"));
}

#[test]
fn teardown_disarms_per_group_condition_cli_guards() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    // Cli (hybrid) materializes one env per (group, condition); `--guard` arms a marker
    // in each. The human runs teardown from the iteration dir, not from inside any env,
    // so the cwd-only disarm never reaches these per-env markers.
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "claude-code",
            "--run-mode",
            "hybrid",
            "--guard",
        ])
        .assert()
        .success();

    let with_marker =
        cli_env_dir(&cwd, "g1", "with_skill").join(".claude/skills/.slow-powers-eval-guard.json");
    let without_marker = cli_env_dir(&cwd, "g1", "without_skill")
        .join(".claude/skills/.slow-powers-eval-guard.json");
    assert!(with_marker.exists());
    assert!(without_marker.exists());

    // Keep the iteration (simulate uncommitted results) so the env dirs survive
    // teardown's reclaim and we can assert the markers themselves were disarmed.
    fs::write(
        iteration_dir(&cwd).join("benchmark.json"),
        "{\"delta\":{\"pass_rate\":0.4}}\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["teardown", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--run-mode", "hybrid"])
        .assert()
        .success()
        .stdout(contains("write guard disarmed"));

    assert!(
        iteration_dir(&cwd).exists(),
        "iteration kept (uncommitted results)"
    );
    assert!(!with_marker.exists(), "with_skill env guard disarmed");
    assert!(!without_marker.exists(), "without_skill env guard disarmed");
}

#[test]
fn finalize_warns_about_armed_cli_per_env_guard() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);

    // Cli (hybrid) arms a guard in each per-(group, condition) env. finalize runs from
    // the iteration dir, not an env, so the cwd-only check misses them; it must walk the
    // per-env markers and remind the operator.
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "claude-code",
            "--run-mode",
            "hybrid",
            "--guard",
        ])
        .assert()
        .success();

    skill_eval()
        .current_dir(&cwd)
        .args(["finalize", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--run-mode",
            "hybrid",
            "--iteration",
            "1",
        ])
        .assert()
        .success()
        .stdout(contains("Guard still armed"));
}
