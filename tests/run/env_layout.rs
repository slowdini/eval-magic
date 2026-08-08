//! Isolated-run env builder: staging redirects into the per-`(group, condition)`
//! `env-<group>-<condition>/` dirs, fixtures are copied into each like a real repo,
//! and `RUNBOOK.md` lives above them in `iteration-N/`. eval-magic meta stays above
//! the envs in `iteration-N/`.

use crate::helpers::*;
use serde_json::json;
use std::collections::HashSet;
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
fn files_root_mounts_nested_fixture_sources_at_task_root() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review", "expected_output": "a review",
          "files_root": "fixtures/todo-app",
          "files": ["package.json", "src/hooks/useDebounce.ts"] } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);
    let source_root = skill_dir.join("mr-review/evals/fixtures/todo-app");
    fs::create_dir_all(source_root.join("src/hooks")).unwrap();
    fs::write(source_root.join("package.json"), "{}").unwrap();
    fs::write(
        source_root.join("src/hooks/useDebounce.ts"),
        "export function useDebounce() {}",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    for cond in ["with_skill", "without_skill"] {
        let env = cli_env_dir(&cwd, "g1", cond);
        assert_eq!(read_str(&env.join("package.json")), "{}");
        assert_eq!(
            read_str(&env.join("src/hooks/useDebounce.ts")),
            "export function useDebounce() {}"
        );
        assert!(!env.join("fixtures").exists());
    }

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let task = dispatch["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["condition"] == "with_skill")
        .unwrap();
    assert_eq!(
        task["fixtures"],
        json!(["package.json", "src/hooks/useDebounce.ts"])
    );
    let prompt = read_str(Path::new(task["dispatch_prompt_path"].as_str().unwrap()));
    assert!(prompt.contains("- package.json"));
    assert!(prompt.contains("- src/hooks/useDebounce.ts"));
    assert!(!prompt.contains("fixtures/todo-app"));
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

    // All with_skill tasks precede all without_skill tasks, so a straight
    // top-to-bottom read of tasks[] dispatches condition A's batch, then
    // condition B's — each from its own per-(group, condition) env.
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
fn every_dispatch_has_a_private_env_and_post_guard_diff_baseline() {
    let tmp = tempfile::TempDir::new().unwrap();
    let evals = r#"{ "skill_name": "mr-review", "evals": [
        { "id": "e1", "prompt": "review", "expected_output": "a review" },
        { "id": "e2", "prompt": "review again", "expected_output": "a review" } ] }"#;
    let (skill_dir, cwd) = setup(tmp.path(), evals);

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let tasks = dispatch["tasks"].as_array().unwrap();
    let roots: HashSet<&str> = tasks
        .iter()
        .map(|task| task["eval_root"].as_str().unwrap())
        .collect();
    assert_eq!(
        roots.len(),
        tasks.len(),
        "every task must own its eval_root"
    );

    for task in tasks {
        let run_dir = Path::new(task["run_record_path"].as_str().unwrap())
            .parent()
            .unwrap();
        let manifest = read_json(&run_dir.join("diff-scope-baseline/manifest.json"));
        assert!(
            manifest["preexisting_files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path
                    .as_str()
                    .unwrap()
                    .ends_with(".slow-powers-eval-guard.json")),
            "baseline must be captured after guard installation: {manifest}"
        );
    }
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
        // Framework artifacts live under the private task env's hidden output
        // subtree; ordinary task edits may live elsewhere in the same env.
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
fn fixture_is_copied_into_each_private_run_environment() {
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

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let tasks = dispatch["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 4, "1 eval × 2 conditions × 2 runs");
    for task in tasks {
        let eval_root = Path::new(task["eval_root"].as_str().unwrap());
        assert_eq!(read_str(&eval_root.join("fixture.txt")), "DATA");
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

    // The declaration is valid, and each task gets an independent copy.
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
        assert_eq!(
            read_str(&Path::new(task["eval_root"].as_str().unwrap()).join("shared.txt")),
            "SHARED"
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
fn guard_marker_scopes_allowed_roots_to_private_env() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--guard"])
        .assert()
        .success();

    // The guard boundary is exactly the private task env. The iteration metadata
    // tree above it and the host temp directory that contains this test are not
    // independently writable roots.
    let env = cli_env_dir(&cwd, "g1", "with_skill");
    let marker = read_json(&env.join(".claude/skills/.slow-powers-eval-guard.json"));
    let roots: Vec<String> = marker["allowedRoots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|root| root.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        roots,
        vec![
            fs::canonicalize(&env)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        ]
    );

    let iter = fs::canonicalize(iteration_dir(&cwd)).unwrap();
    assert!(
        !roots.iter().any(|root| iter.starts_with(root)),
        "allowedRoots {roots:?} must not cover the meta tree above env at {iter:?}"
    );
}
