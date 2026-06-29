//! Isolated-run env builder: staging redirects into the per-`(group, condition)`
//! `env-<group>-<condition>/` dirs, fixtures are copied into each like a real repo,
//! and `RUNBOOK.md` lives above them in `iteration-N/`. eval-magic meta stays above
//! the envs in `iteration-N/`.

use crate::helpers::*;
use serde_json::json;
use std::fs;
use std::path::Path;

#[test]
fn stages_into_env_not_cwd() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    // The staged skill lands under env-g1-with_skill/.claude/skills, not the
    // invocation cwd.
    assert_eq!(
        env_staged_entries(&cwd),
        vec!["slow-powers-eval-1-with_skill__mr-review"]
    );
    assert!(
        !cwd.join(".claude/skills").exists(),
        "nothing should be staged at the invocation cwd anymore"
    );
    // eval-magic meta stays above the env, in iteration-N/.
    assert!(iteration_dir(&cwd).join("dispatch.json").exists());
    assert!(
        !cli_env_dir(&cwd, "g1", "with_skill")
            .join("dispatch.json")
            .exists()
    );
}

#[test]
fn env_dir_created_even_with_no_stage() {
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
            "--no-stage",
            "--dry-run",
        ])
        .assert()
        .success();

    // Even with staging disabled, each per-(group, condition) env must exist for
    // fixtures + the per-env guard.
    assert!(cli_env_dir(&cwd, "g1", "with_skill").is_dir());
    assert!(cli_env_dir(&cwd, "g1", "without_skill").is_dir());
}

#[test]
fn fixtures_copied_into_env_like_a_real_repo() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review", "expected_output": "a review",
          "files": ["src/main.rs", "data/x.json"] } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    let evals_dir = skill_dir.join("mr-review/evals");
    fs::create_dir_all(evals_dir.join("src")).unwrap();
    fs::create_dir_all(evals_dir.join("data")).unwrap();
    fs::write(evals_dir.join("src/main.rs"), "fn main() {}").unwrap();
    fs::write(evals_dir.join("data/x.json"), "{}").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    // Structure preserved under each per-condition env, not flattened into an
    // inputs/ bucket. Fixtures are copied into every relevant env (per its group).
    for cond in ["with_skill", "without_skill"] {
        let env = cli_env_dir(&cwd, "g1", cond);
        assert_eq!(read_str(&env.join("src/main.rs")), "fn main() {}");
        assert_eq!(read_str(&env.join("data/x.json")), "{}");
        assert!(!env.join("inputs").exists());
    }

    // The dispatch prompt lists fixtures env-relative — the agent's cwd is env.
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["condition"] == "with_skill")
        .unwrap();
    let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
    assert!(prompt.contains("- src/main.rs"));
    assert!(prompt.contains("- data/x.json"));
    assert!(!prompt.contains("inputs/"));
}

#[test]
fn dispatch_tasks_grouped_by_condition() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Two evals so the interleaved-vs-grouped distinction is observable.
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review", "expected_output": "a review" },
        { "id": "e2", "prompt": "review again", "expected_output": "a review" } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let conds: Vec<String> = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["condition"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(conds.len(), 4, "2 evals × 2 conditions: {conds:?}");

    // All with_skill tasks precede all without_skill tasks, so the runbook's
    // "dispatch all of cond A → switch-condition → dispatch all of cond B" batches
    // map to a straight top-to-bottom read of tasks[].
    let first_b = conds.iter().position(|c| c == "without_skill").unwrap();
    assert!(
        conds[..first_b].iter().all(|c| c == "with_skill"),
        "cond A not contiguous at the front: {conds:?}"
    );
    assert!(
        conds[first_b..].iter().all(|c| c == "without_skill"),
        "cond B not contiguous at the back: {conds:?}"
    );
}

#[test]
fn dispatch_outputs_live_under_env() {
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
    let tasks = dispatch["tasks"].as_array().unwrap();
    assert!(!tasks.is_empty(), "run produced dispatch tasks");

    // Canonicalize to compare across the macOS /var → /private/var symlink:
    // dispatch.json stores resolved paths, but the test roots come from the raw
    // tempdir, so a lexical starts_with would mismatch.
    let iter = fs::canonicalize(iteration_dir(&cwd)).unwrap();
    for task in tasks {
        // The agent-under-test (cwd = its per-(group, condition) env) writes only
        // inside that env's .eval-magic-outputs/.
        let cond = task["condition"].as_str().unwrap();
        let env = fs::canonicalize(cli_env_dir(&cwd, "g1", cond)).unwrap();
        let outputs_root = env.join(".eval-magic-outputs");
        let outputs_dir = fs::canonicalize(task["outputs_dir"].as_str().unwrap()).unwrap();
        assert!(
            outputs_dir.starts_with(&outputs_root),
            "outputs_dir under env-g1-{cond}/.eval-magic-outputs/: {}",
            outputs_dir.display()
        );
        // run.json / timing.json are eval-magic meta: above the env, in iteration-N/.
        // The files don't exist yet (dry-run), so canonicalize their shared run dir.
        let run_record = Path::new(task["run_record_path"].as_str().unwrap());
        let timing = Path::new(task["timing_path"].as_str().unwrap());
        let run_meta_dir = fs::canonicalize(run_record.parent().unwrap()).unwrap();
        assert!(
            run_meta_dir.starts_with(&iter) && !run_meta_dir.starts_with(&env),
            "run dir stays above env: {}",
            run_meta_dir.display()
        );
        assert_eq!(
            timing.parent().unwrap(),
            run_record.parent().unwrap(),
            "run.json and timing.json share the meta run dir"
        );
    }
}

#[test]
fn shared_fixture_copied_once_across_conditions_and_runs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review", "expected_output": "a review",
          "files": ["fixture.txt"] } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    fs::write(skill_dir.join("mr-review/evals/fixture.txt"), "DATA").unwrap();

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
        .success();

    // One copy per env, shared env-relative by that env's runs. Each condition env
    // carries its own copy, referenced env-relative ("fixture.txt") by every task.
    for cond in ["with_skill", "without_skill"] {
        assert_eq!(
            read_str(&cli_env_dir(&cwd, "g1", cond).join("fixture.txt")),
            "DATA"
        );
    }
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let tasks = dispatch["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 4, "1 eval × 2 conditions × 2 runs");
    for task in tasks {
        assert_eq!(
            task["fixtures"].as_array().unwrap(),
            &vec![json!("fixture.txt")]
        );
    }
}

#[test]
fn two_evals_sharing_a_fixture_declaration_succeeds() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "p1", "expected_output": "o", "files": ["shared.txt"] },
        { "id": "e2", "prompt": "p2", "expected_output": "o", "files": ["shared.txt"] } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    fs::write(skill_dir.join("mr-review/evals/shared.txt"), "SHARED").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    // Two evals declaring the same fixture from the same source is an idempotent
    // share: the with_skill env carries a single copy.
    assert_eq!(
        read_str(&cli_env_dir(&cwd, "g1", "with_skill").join("shared.txt")),
        "SHARED"
    );
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for id in ["e1", "e2"] {
        let task = dispatch["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["eval_id"] == id && t["condition"] == "with_skill")
            .unwrap();
        assert_eq!(
            task["fixtures"].as_array().unwrap(),
            &vec![json!("shared.txt")]
        );
    }
}

#[test]
fn env_contains_only_the_staged_skill_no_repo_leakage() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    // A stray skill sitting in the invocation cwd's .claude/skills must NOT leak into env:
    // read isolation comes from env being a clean, separate cwd.
    fs::create_dir_all(cwd.join(".claude/skills/unrelated-skill")).unwrap();
    fs::write(
        cwd.join(".claude/skills/unrelated-skill/SKILL.md"),
        "---\nname: unrelated-skill\ndescription: leaked\n---\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    // env-g1-with_skill/.claude/skills holds only the staged skill-under-test.
    assert_eq!(
        env_staged_entries(&cwd),
        vec!["slow-powers-eval-1-with_skill__mr-review"]
    );
    // The unrelated cwd skill is absent from the env.
    assert!(
        !cli_env_dir(&cwd, "g1", "with_skill")
            .join(".claude/skills/unrelated-skill")
            .exists()
    );
}

#[test]
fn guard_marker_allowed_roots_cover_meta_above_env() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();

    // The guard arms inside each env, but its allowedRoots include the workspace root
    // above env, so eval-magic can still write meta (benchmark.json, dispatch.json)
    // into iteration-N/.
    let marker = read_json(
        &cli_env_dir(&cwd, "g1", "with_skill").join(".claude/skills/.slow-powers-eval-guard.json"),
    );
    let roots = marker["allowedRoots"].as_array().unwrap();
    let iter = iteration_dir(&cwd);
    assert!(
        roots.iter().any(|r| iter.starts_with(r.as_str().unwrap())),
        "allowedRoots {roots:?} must cover the meta tree above env at {iter:?}"
    );
}
