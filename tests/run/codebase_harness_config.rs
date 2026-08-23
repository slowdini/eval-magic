//! Cross-harness project-skill preservation, exclusion, and collision behavior.

use crate::codebase_support::{
    add_project_skill_roots, codebase_repo, commit, evals_with_codebase,
};
use crate::helpers::*;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn sourced_project_skills_and_instructions_are_preserved_by_default_for_every_harness() {
    for (harness, roots) in [
        ("claude-code", vec![".claude/skills"]),
        ("cline", vec![".cline/skills"]),
        ("codex", vec![".agents/skills"]),
        (
            "opencode",
            vec![".opencode/skills", ".claude/skills", ".agents/skills"],
        ),
    ] {
        let tmp = tempfile::TempDir::new().unwrap();
        let origin = codebase_repo(tmp.path(), "origin", "main");
        add_project_skill_roots(&origin, &roots);
        let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
        let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
        fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

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
                harness,
                "--no-guard",
                "--dry-run",
            ])
            .assert()
            .success();

        for condition in ["with_skill", "without_skill"] {
            let env = cli_env_dir(&cwd, "g1", condition);
            for root in &roots {
                assert!(
                    env.join(root).join("mr-review/SKILL.md").exists(),
                    "{harness}/{condition}: {root} subject source was removed"
                );
                assert!(
                    env.join(root)
                        .join("slow-powers-eval-codebase-owned/SKILL.md")
                        .exists(),
                    "{harness}/{condition}: a codebase-owned prefixed skill was removed"
                );
            }
            assert_eq!(
                fs::read_to_string(env.join("CLAUDE.md")).unwrap(),
                "claude instructions\n"
            );
            assert_eq!(
                fs::read_to_string(env.join("AGENTS.md")).unwrap(),
                "agent instructions\n"
            );
        }

        let shadow = read_json(&iteration_dir(&cwd).join("plugin-shadow.json"));
        let codebase_findings: Vec<_> = shadow["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|finding| finding["class"] == "codebase-sourced")
            .collect();
        assert_eq!(
            codebase_findings.len(),
            1,
            "{harness}: expected one grouped codebase finding: {shadow}"
        );
        assert_eq!(codebase_findings[0]["skill_name"], "mr-review");
        let live_sources: Vec<_> = codebase_findings[0]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|source| source["origin"] == "live")
            .collect();
        assert_eq!(
            live_sources.len(),
            roots.len() * 2,
            "{harness}: every project root should appear in both task environments"
        );
        assert!(
            live_sources
                .iter()
                .all(|source| source["appearances"].as_array().unwrap().len() == 1),
            "{harness}: each concrete environment source should name its own cell"
        );
    }
}

#[test]
fn opencode_excludes_all_project_skill_sources_under_no_stage_but_keeps_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let roots = [".opencode/skills", ".claude/skills", ".agents/skills"];
    add_project_skill_roots(&origin, &roots);
    let source = format!(
        r#"{{ "url": "{}", "ref": "main", "exclude_skill_sources": true }}"#,
        wire_path(&origin)
    );
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

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
            "--no-guard",
            "--dry-run",
        ])
        .assert()
        .success();

    for condition in ["with_skill", "without_skill"] {
        let env = cli_env_dir(&cwd, "g1", condition);
        for root in roots {
            assert!(
                !env.join(root).join("mr-review").exists(),
                "{condition}: {root} remained discoverable"
            );
            assert!(
                !env.join(root)
                    .join("slow-powers-eval-codebase-owned")
                    .exists(),
                "{condition}: {root} retained another codebase skill"
            );
        }
        assert_eq!(
            fs::read_to_string(env.join("CLAUDE.md")).unwrap(),
            "claude instructions\n"
        );
        assert_eq!(
            fs::read_to_string(env.join("AGENTS.md")).unwrap(),
            "agent instructions\n"
        );
        assert_eq!(
            fs::read_to_string(env.join(".opencode/settings.json")).unwrap(),
            "{}\n"
        );
    }

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    assert_eq!(conditions["codebases"][0]["exclude_skill_sources"], true);
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert!(
        dispatch["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| task["codebase"]["exclude_skill_sources"] == true)
    );
    let backup_roots: Vec<PathBuf> = ["with_skill", "without_skill"]
        .into_iter()
        .flat_map(|condition| {
            let manifest = read_json(
                &cli_env_dir(&cwd, "g1", condition)
                    .join(".opencode/skills")
                    .join(STAGED_MANIFEST),
            );
            manifest["excluded_roots"]
                .as_array()
                .unwrap()
                .iter()
                .map(|root| {
                    Path::new(root["backup_path"].as_str().unwrap())
                        .parent()
                        .unwrap()
                        .to_path_buf()
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let shadow_path = iteration_dir(&cwd).join("plugin-shadow.json");
    if shadow_path.exists() {
        let shadow = read_json(&shadow_path);
        assert!(
            shadow["findings"]
                .as_array()
                .unwrap()
                .iter()
                .all(|finding| finding["class"] != "codebase-sourced"),
            "excluded project roots must not produce codebase findings: {shadow}"
        );
    }

    skill_eval()
        .current_dir(&cwd)
        .args(["teardown", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "opencode"])
        .assert()
        .success();
    assert!(
        backup_roots.iter().all(|root| !root.exists()),
        "teardown left exclusion backups behind: {backup_roots:?}"
    );
}

#[test]
fn generated_slug_collision_is_displaced_only_in_the_staged_arm() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let slug = "slow-powers-eval-1-with_skill__mr-review";
    fs::create_dir_all(origin.join(".claude/skills").join(slug)).unwrap();
    fs::write(
        origin.join(".claude/skills").join(slug).join("SKILL.md"),
        "---\nname: mr-review\ndescription: codebase collision\n---\n\nCODEBASE\n",
    )
    .unwrap();
    commit(&origin, "add exact staged-slug collision");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "new-skill",
            "--no-guard",
            "--dry-run",
        ])
        .assert()
        .success();

    let with_root = cli_env_dir(&cwd, "g1", "with_skill");
    let without_root = cli_env_dir(&cwd, "g1", "without_skill");
    assert!(
        fs::read_to_string(with_root.join(".claude/skills").join(slug).join("SKILL.md"))
            .unwrap()
            .contains("body"),
        "the staged arm should contain the evaluated skill"
    );
    assert!(
        fs::read_to_string(
            without_root
                .join(".claude/skills")
                .join(slug)
                .join("SKILL.md")
        )
        .unwrap()
        .contains("CODEBASE"),
        "the control arm should retain the sourced copy"
    );
    let manifest = read_json(&with_root.join(".claude/skills").join(STAGED_MANIFEST));
    let staged_entry = manifest["created_entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == slug)
        .unwrap();
    assert_eq!(staged_entry["preexisting"], true);

    let shadow = read_json(&iteration_dir(&cwd).join("plugin-shadow.json"));
    let finding = shadow["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["class"] == "codebase-sourced")
        .unwrap();
    let live_sources: Vec<_> = finding["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|source| source["origin"] == "live")
        .collect();
    assert_eq!(live_sources.len(), 1, "{finding}");
    assert_eq!(
        live_sources[0]["appearances"][0]["condition"],
        "without_skill"
    );
}

#[test]
fn revision_mode_excludes_codebase_skill_sources_from_both_arms() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    add_project_skill_roots(&origin, &[".claude/skills"]);
    let source = format!(
        r#"{{ "url": "{}", "ref": "main", "exclude_skill_sources": true }}"#,
        wire_path(&origin)
    );
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["snapshot", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--label", "baseline"])
        .assert()
        .success();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--mode",
            "revision",
            "--no-guard",
            "--dry-run",
        ])
        .assert()
        .success();

    for condition in ["old_skill", "new_skill"] {
        let env = iteration_dir(&cwd).join(format!("env-g1-{condition}"));
        assert!(
            !env.join(".claude/skills/mr-review").exists(),
            "{condition}: the codebase subject source remained discoverable"
        );
        let staged = staged_entries(&env.join(".claude/skills"));
        assert_eq!(staged.len(), 1, "{condition}: {staged:?}");
        assert!(staged[0].contains(condition), "{condition}: {staged:?}");
        assert_eq!(
            fs::read_to_string(env.join("CLAUDE.md")).unwrap(),
            "claude instructions\n"
        );
        assert_eq!(
            fs::read_to_string(env.join("AGENTS.md")).unwrap(),
            "agent instructions\n"
        );
    }

    let shadow_path = iteration_dir(&cwd).join("plugin-shadow.json");
    if shadow_path.exists() {
        let shadow = read_json(&shadow_path);
        assert!(
            shadow["findings"]
                .as_array()
                .unwrap()
                .iter()
                .all(|finding| finding["class"] != "codebase-sourced"),
            "excluded revision arms must not report codebase sources: {shadow}"
        );
    }
}

#[test]
fn exclusion_is_a_noop_for_a_byoh_harness_without_project_skill_roots() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let source = format!(
        r#"{{ "url": "{}", "ref": "main", "exclude_skill_sources": true }}"#,
        wire_path(&origin)
    );
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();
    let harness_dir = cwd.join(".eval-magic/harnesses");
    fs::create_dir_all(&harness_dir).unwrap();
    fs::write(
        harness_dir.join("cool.toml"),
        "label = \"cool-custom-harness\"\n",
    )
    .unwrap();

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
            "cool-custom-harness",
            "--no-guard",
            "--dry-run",
        ])
        .assert()
        .success();

    for condition in ["with_skill", "without_skill"] {
        assert!(
            cli_env_dir(&cwd, "g1", condition)
                .join("src/lib.rs")
                .exists()
        );
    }
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    assert!(
        dispatch["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| task["codebase"]["exclude_skill_sources"] == true)
    );
}
