//! Sourcing a real codebase into each task environment (issue #252).
//!
//! The environments a run builds are asserted here rather than in unit tests
//! because the property under test spans resolution, provisioning, staging, and
//! the fixture overlay — it is only true of a whole prepared workspace.

use crate::codebase_support::{
    an_object_file, codebase_repo, commit, evals_with_codebase, git, link_count,
};
use crate::helpers::*;
use std::fs;

#[test]
fn a_git_codebase_arrives_in_every_env_with_history_and_no_remote() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(
        skill_dir.join("mr-review/evals/TASK.md"),
        "Add a `two()` function.\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    for condition in ["with_skill", "without_skill"] {
        let env = cli_env_dir(&cwd, "g1", condition);
        assert_eq!(
            fs::read_to_string(env.join("src/main.rs")).unwrap(),
            "fn main() {}\n",
            "{condition}: the codebase's files must be present"
        );
        assert!(
            git(&env, &["rev-list", "--count", "HEAD"])
                .parse::<u32>()
                .unwrap()
                >= 2,
            "{condition}: the codebase's history must survive provisioning"
        );
        assert_eq!(
            git(&env, &["remote"]),
            "",
            "{condition}: no env may retain a remote"
        );
        // The overlay: a declared fixture lands on top, at its declared path.
        assert_eq!(
            fs::read_to_string(env.join("TASK.md")).unwrap(),
            "Add a `two()` function.\n",
            "{condition}: files are an overlay on the codebase"
        );
    }
}

#[test]
fn the_baseline_ref_marks_the_state_every_codebase_env_starts_from() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let env = cli_env_dir(&cwd, "g1", "with_skill");
    assert_eq!(
        git(&env, &["rev-parse", "refs/eval-magic/baseline"]),
        git(&env, &["rev-parse", "HEAD"]),
        "the baseline ref must name the start state"
    );
    // Outside refs/heads, so it never shows up in what the agent sees.
    assert_eq!(git(&env, &["branch", "--list"]), "* main");
    assert_eq!(
        git(&env, &["status", "--porcelain"]),
        "",
        "the baseline commit must leave nothing uncommitted"
    );
}

/// A real repository ignores its build output. Committing that into the
/// baseline would put megabytes of artifacts in every environment's start
/// state — but the runner's own files have to land regardless of what the
/// codebase ignores, or the condition under test falls outside every diff.
#[test]
fn the_baseline_respects_codebase_gitignore_but_still_tracks_runner_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    // The codebase ignores the harness config dir the runner stages into.
    fs::write(origin.join(".gitignore"), "build/\n.claude/\n").unwrap();
    commit(&origin, "ignore the harness config dir too");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let env = cli_env_dir(&cwd, "g1", "with_skill");
    let tracked = git(&env, &["ls-files"]);

    assert!(
        !tracked.lines().any(|path| path.starts_with("build/")),
        "gitignored build output must stay out of the baseline:\n{tracked}"
    );
    assert!(
        tracked.lines().any(|path| path.starts_with(".claude/")),
        "the staged skill must be tracked even though the codebase ignores .claude/:\n{tracked}"
    );
    assert!(
        tracked.lines().any(|path| path == "TASK.md"),
        "the fixture overlay must be tracked:\n{tracked}"
    );
    assert!(
        !tracked
            .lines()
            .any(|path| path.starts_with(".eval-magic-outputs")),
        "framework output stays excluded:\n{tracked}"
    );
}

/// Sourcing a codebase runs git against a URL the operator supplied, on a host
/// whose git configuration the operator also controls. `insteadOf` rewrites that
/// URL, so a leak here would silently source a *different* tree than the one the
/// eval declared — and the report would still cite the declared one.
///
/// Injected through `GIT_CONFIG_COUNT` because that is the one mechanism a test
/// can use without writing to the developer's real `~/.gitconfig`.
#[test]
fn sourcing_a_codebase_ignores_the_operators_git_configuration() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        // Rewrites the codebase URL to somewhere that does not resolve.
        .env("GIT_CONFIG_COUNT", "1")
        .env(
            "GIT_CONFIG_KEY_0",
            "url.https://eval-magic.invalid/.insteadOf",
        )
        .env("GIT_CONFIG_VALUE_0", wire_path(&origin))
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let env = cli_env_dir(&cwd, "g1", "with_skill");
    assert_eq!(
        fs::read_to_string(env.join("src/main.rs")).unwrap(),
        "fn main() {}\n",
        "the declared codebase must be the one sourced"
    );
}

/// A report that cites a codebase has to say *which* tree it measured. The
/// declared ref is not enough — a branch moves — so the resolved commit is what
/// every provenance surface carries.
#[test]
fn the_resolved_codebase_reaches_conditions_and_every_dispatch_task() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let revision = git(&origin, &["rev-parse", "HEAD"]);
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    let codebases = conditions["codebases"].as_array().unwrap();
    assert_eq!(codebases.len(), 1, "one declared codebase, resolved once");
    let recorded = &codebases[0];
    assert_eq!(recorded["kind"], "git");
    assert_eq!(recorded["source"], wire_path(&origin));
    assert_eq!(recorded["ref"], "main");
    assert_eq!(recorded["revision"], revision);
    assert_eq!(recorded["branch"], "main");
    assert_eq!(recorded["evals"][0], "e1");
    assert!(
        recorded.get("host_local").is_none(),
        "a git url is reproducible, so the flag stays off the artifact"
    );

    // Every dispatch task carries it, which is how it reaches each run.json.
    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    let tasks = dispatch["tasks"].as_array().unwrap();
    assert!(!tasks.is_empty());
    for task in tasks {
        assert_eq!(
            task["codebase"]["revision"], revision,
            "each task records the tree it ran against"
        );
    }
}

/// A `path` source cannot be resolved by anyone else — a different machine has
/// the directory somewhere else, or nowhere. That is unfixable, so the artifact
/// says so rather than implying a reproducibility it does not have.
#[test]
fn a_path_codebase_is_recorded_as_host_local_with_its_origin_for_citation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let upstream = codebase_repo(tmp.path(), "upstream", "main");
    let local = codebase_repo(tmp.path(), "local", "main");
    // Git stores a remote URL byte-for-byte, and eval-magic cites it unchanged
    // rather than rewriting what a user configured.
    let origin_url = upstream.to_string_lossy().to_string();
    git(&local, &["remote", "add", "origin", &origin_url]);
    let revision = git(&local, &["rev-parse", "HEAD"]);
    let source = format!(r#"{{ "path": "{}" }}"#, wire_path(&local));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(skill_dir.join("mr-review/evals/TASK.md"), "task\n").unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    let conditions = read_json(&iteration_dir(&cwd).join("conditions.json"));
    let recorded = &conditions["codebases"][0];
    assert_eq!(recorded["kind"], "path");
    assert_eq!(recorded["host_local"], true);
    assert_eq!(recorded["revision"], revision);
    // What makes it citable anyway: origin + revision resolve anywhere.
    assert_eq!(recorded["origin_url"], origin_url);
}

/// One cached materialization provisions every environment of a
/// multi-run campaign, and the provisioning is a local clone — each
/// environment's object store is hard-linked to the cache's, not copied.
#[test]
fn multi_run_envs_are_provisioned_from_one_cached_materialization() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(
        skill_dir.join("mr-review/evals/TASK.md"),
        "task
",
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
            "--runs",
            "2",
            "--dry-run",
        ])
        .assert()
        .success();

    // One codebase declaration resolves to one cached materialization, shared
    // by every environment the run provisions.
    let iteration = iteration_dir(&cwd);
    let cached: Vec<_> = fs::read_dir(iteration.join(".codebase")).unwrap().collect();
    assert_eq!(cached.len(), 1, "one codebase, one cached materialization");
    // One object file the cache holds, spelled relative to the cache root — the
    // same content-addressed path every environment provisioned from it holds.
    let cache = cached[0].as_ref().unwrap().path();
    let object = an_object_file(&cache);
    let object_relative = object.strip_prefix(&cache).unwrap();

    for condition in ["with_skill", "without_skill"] {
        for run in [1, 2] {
            let env = iteration.join(format!("env-g1-{condition}-run-{run}"));
            assert_eq!(
                fs::read_to_string(env.join("src/main.rs")).unwrap(),
                "fn main() {}
",
                "{condition} run {run}: the codebase's files must be present"
            );
            assert!(
                git(&env, &["rev-list", "--count", "HEAD"])
                    .parse::<u32>()
                    .unwrap()
                    >= 2,
                "{condition} run {run}: the clone must carry the history"
            );
            assert_eq!(
                git(&env, &["remote"]),
                "",
                "{condition} run {run}: no env may retain a remote, the cache included"
            );
            assert_eq!(
                git(&env, &["rev-parse", "refs/eval-magic/baseline"]),
                git(&env, &["rev-parse", "HEAD"]),
                "{condition} run {run}: the baseline still names the start state"
            );
            assert!(
                link_count(&env.join(object_relative)) >= 2,
                "{condition} run {run}: the object store must be hard-linked to the cache's, not copied byte by byte"
            );
        }
    }
}

/// A codebase that carries no Git history — a plain directory — still yields
/// a working environment end to end. Its cache is a commitless repository,
/// which a local clone could not populate (cloning an empty repository checks
/// out nothing), so provisioning takes the plain-copy fallback.
#[test]
fn a_historyless_codebase_provisions_every_env_through_the_copy_fallback() {
    let tmp = tempfile::TempDir::new().unwrap();
    let plain = tmp.path().join("legacy-service");
    fs::create_dir_all(plain.join("src")).unwrap();
    fs::write(
        plain.join("src/main.rs"),
        "fn main() {}
",
    )
    .unwrap();
    let source = format!(r#"{{ "path": "{}" }}"#, wire_path(&plain));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(
        skill_dir.join("mr-review/evals/TASK.md"),
        "task
",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success();

    for condition in ["with_skill", "without_skill"] {
        let env = cli_env_dir(&cwd, "g1", condition);
        assert_eq!(
            fs::read_to_string(env.join("src/main.rs")).unwrap(),
            "fn main() {}
",
            "{condition}: a historyless codebase must still populate the working tree"
        );
        assert_eq!(
            git(&env, &["remote"]),
            "",
            "{condition}: no env may retain a remote"
        );
        assert_eq!(
            git(&env, &["rev-parse", "refs/eval-magic/baseline"]),
            git(&env, &["rev-parse", "HEAD"]),
            "{condition}: the baseline still names the start state"
        );
    }
}

/// The run plan names each codebase and the commit it resolved to, and says the
/// iteration materializes it once — the fact #254 turned into a guarantee.
#[test]
fn the_run_plan_names_the_codebase_and_its_resolved_commit() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let revision = git(&origin, &["rev-parse", "HEAD"]);
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(
        skill_dir.join("mr-review/evals/TASK.md"),
        "task
",
    )
    .unwrap();

    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--mode", "new-skill", "--dry-run"])
        .assert()
        .success()
        .stdout(predicates::str::contains("codebase: "))
        .stdout(predicates::str::contains(&revision[..7]))
        .stdout(predicates::str::contains("materialized once"));
}

/// Mode B parity: a revision-mode run provisions both arms of the comparison
/// from the same cached codebase, so a skill edit is measured against the same
/// tree the previous skill ran on.
#[test]
fn revision_mode_provisions_both_arms_from_the_cached_codebase() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = codebase_repo(tmp.path(), "origin", "main");
    let source = format!(r#"{{ "url": "{}", "ref": "main" }}"#, wire_path(&origin));
    let (skill_dir, cwd) = setup(tmp.path(), &evals_with_codebase(&source));
    fs::write(
        skill_dir.join("mr-review/evals/TASK.md"),
        "task
",
    )
    .unwrap();

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
        .args(["--skill", "mr-review", "--mode", "revision", "--dry-run"])
        .assert()
        .success();

    let iteration = iteration_dir(&cwd);
    let cached: Vec<_> = fs::read_dir(iteration.join(".codebase")).unwrap().collect();
    assert_eq!(
        cached.len(),
        1,
        "both arms of the comparison share one cached materialization"
    );

    for condition in ["old_skill", "new_skill"] {
        let env = iteration.join(format!("env-g1-{condition}"));
        assert_eq!(
            fs::read_to_string(env.join("src/main.rs")).unwrap(),
            "fn main() {}
",
            "{condition}: the codebase's files must be present"
        );
        assert!(
            git(&env, &["rev-list", "--count", "HEAD"])
                .parse::<u32>()
                .unwrap()
                >= 2,
            "{condition}: the history must survive provisioning"
        );
        assert_eq!(
            git(&env, &["remote"]),
            "",
            "{condition}: no env may retain a remote"
        );
        assert_eq!(
            git(&env, &["rev-parse", "refs/eval-magic/baseline"]),
            git(&env, &["rev-parse", "HEAD"]),
            "{condition}: the baseline still names the start state"
        );
    }
}
