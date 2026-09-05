use super::*;
use serde_json::Value;
use tempfile::TempDir;

mod guard_provenance;

/// Write `body` to `path`, creating parent dirs.
fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

struct Fixture {
    _tmp: TempDir,
    skill_subdir: PathBuf,
    workspace_root: PathBuf,
    iteration_dir: PathBuf,
}

/// Build a skill dir (with SKILL.md) and a workspace iteration dir.
fn fixture(iteration: u32) -> Fixture {
    let tmp = TempDir::new().unwrap();
    let skill_subdir = tmp.path().join("skill-dir").join("mr-review");
    write(
        &skill_subdir.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
    );
    let workspace_root = tmp.path().join("work").join(".eval-magic");
    let iteration_dir = workspace_root
        .join("mr-review")
        .join(format!("iteration-{iteration}"));
    fs::create_dir_all(&iteration_dir).unwrap();
    Fixture {
        _tmp: tmp,
        skill_subdir,
        workspace_root,
        iteration_dir,
    }
}

fn opts<'a>(f: &'a Fixture, iteration: u32) -> PromoteOptions<'a> {
    PromoteOptions {
        workspace_root: &f.workspace_root,
        skill_name: "mr-review",
        skill_subdir: &f.skill_subdir,
        iteration,
        harness: Harness::resolve("claude-code").unwrap(),
        label: None,
        agent_model: None,
        judge_model: None,
        responder_model: None,
    }
}

const CONDITIONS: &str = r#"{
  "mode": "new-skill",
  "conditions": [
    { "name": "with_skill", "skill_path": "/x/SKILL.md" },
    { "name": "without_skill", "skill_path": null }
  ],
  "timestamp": "2026-05-27T00:00:00.000Z",
  "harness": "claude-code"
}"#;

#[test]
fn copies_benchmark_and_per_run_gradings_into_baseline() {
    let f = fixture(2);
    write(&f.iteration_dir.join("conditions.json"), CONDITIONS);
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0.5}}"#,
    );
    write(
        &f.iteration_dir.join("eval-e1/with_skill/grading.json"),
        r#"{"summary":{"pass_rate":1}}"#,
    );
    write(
        &f.iteration_dir.join("eval-e1/without_skill/grading.json"),
        r#"{"summary":{"pass_rate":0}}"#,
    );

    let res = promote_baseline(&opts(&f, 2)).unwrap();
    let baseline = &res.baseline_dir;

    assert_eq!(res.gradings_copied, 2);
    let benchmark = fs::read_to_string(baseline.join("benchmark.json")).unwrap();
    assert!(benchmark.contains("\"pass_rate\":0.5"));
    let with = fs::read_to_string(baseline.join("grading/e1__with_skill.json")).unwrap();
    assert!(with.contains("\"pass_rate\":1"));
    assert!(baseline.join("grading/e1__without_skill.json").exists());

    let provenance = fs::read_to_string(baseline.join("BASELINE.md")).unwrap();
    assert!(provenance.contains("new-skill"));
    assert!(provenance.contains("iteration-2"));
    assert!(provenance.contains("claude-code"));
    assert!(provenance.contains("2026-05-27T00:00:00.000Z"));
    assert!(provenance.contains("Agent model | unspecified"));
    assert!(provenance.contains("Judge model | unspecified"));
    assert!(provenance.contains("Responder model | unspecified"));
    assert!(provenance.contains("per-assertion pass or sampled-vote counts"));
}

#[test]
fn captures_per_run_gradings_for_multi_run_cells() {
    let f = fixture(4);
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0.5}}"#,
    );
    // eval-e1: runs=3 → gradings nested under run-<k>/.
    for cond in ["with_skill", "without_skill"] {
        for k in 1..=3 {
            write(
                &f.iteration_dir
                    .join(format!("eval-e1/{cond}/run-{k}/grading.json")),
                r#"{"summary":{"pass_rate":1}}"#,
            );
        }
    }
    // eval-e2: runs=1 → flat legacy layout.
    write(
        &f.iteration_dir.join("eval-e2/with_skill/grading.json"),
        r#"{"summary":{"pass_rate":0}}"#,
    );

    let res = promote_baseline(&opts(&f, 4)).unwrap();
    let baseline = &res.baseline_dir;

    assert_eq!(res.gradings_copied, 7);
    // Nested cells carry an __r<k> suffix per run.
    for k in 1..=3 {
        assert!(
            baseline
                .join(format!("grading/e1__with_skill__r{k}.json"))
                .exists()
        );
        assert!(
            baseline
                .join(format!("grading/e1__without_skill__r{k}.json"))
                .exists()
        );
    }
    // The flat runs=1 cell keeps the unsuffixed name.
    assert!(baseline.join("grading/e2__with_skill.json").exists());
    assert_eq!(res.missing_gradings, 0);
}

#[test]
fn reports_missing_gradings_for_incomplete_run_cells() {
    let f = fixture(5);
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );
    // run-1 graded; run-2 dispatched but never graded (incomplete iteration).
    write(
        &f.iteration_dir
            .join("eval-e1/with_skill/run-1/grading.json"),
        r#"{"summary":{"pass_rate":1}}"#,
    );
    fs::create_dir_all(f.iteration_dir.join("eval-e1/with_skill/run-2")).unwrap();

    let res = promote_baseline(&opts(&f, 5)).unwrap();

    assert_eq!(res.gradings_copied, 1);
    assert_eq!(res.missing_gradings, 1);
}

#[test]
fn drops_promoted_marker_into_iteration_dir() {
    let f = fixture(3);
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    promote_baseline(&opts(&f, 3)).unwrap();

    let marker_path = f.iteration_dir.join(PROMOTED_MARKER);
    assert!(marker_path.exists());
    let marker: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&marker_path).unwrap()).unwrap();
    assert!(
        marker["promoted_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert_eq!(
        marker["baseline_dir"].as_str().unwrap(),
        f.skill_subdir
            .join("evals")
            .join("baseline")
            .to_string_lossy()
    );
}

#[test]
fn records_agent_and_judge_models_when_provided() {
    let f = fixture(1);
    write(&f.iteration_dir.join("conditions.json"), CONDITIONS);
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    let mut o = opts(&f, 1);
    o.agent_model = Some("claude-haiku-4-5-20251001");
    o.judge_model = Some("claude-opus-4-7");
    o.responder_model = Some("claude-haiku-4-5-20251001");
    promote_baseline(&o).unwrap();

    let provenance = fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
    assert!(provenance.contains("Agent model | claude-haiku-4-5-20251001"));
    assert!(provenance.contains("Judge model | claude-opus-4-7"));
    assert!(provenance.contains("Responder model | claude-haiku-4-5-20251001"));
}

const CONDITIONS_WITH_PROVENANCE: &str = r#"{
  "mode": "new-skill",
  "conditions": [
    { "name": "with_skill", "skill_path": "/x/SKILL.md" },
    { "name": "without_skill", "skill_path": null }
  ],
  "timestamp": "2026-05-27T00:00:00.000Z",
  "harness": "claude-code",
  "agent_model": "claude-haiku-4-5-20251001",
  "judge_model": "claude-opus-4-8",
  "responder_model": "claude-haiku-4-5-20251001",
  "label": "canonical-run"
}"#;

#[test]
fn provenance_falls_back_to_manifest_models_and_label() {
    let f = fixture(1);
    write(
        &f.iteration_dir.join("conditions.json"),
        CONDITIONS_WITH_PROVENANCE,
    );
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    promote_baseline(&opts(&f, 1)).unwrap();

    let provenance = fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
    assert!(provenance.contains("Agent model | claude-haiku-4-5-20251001"));
    assert!(provenance.contains("Judge model | claude-opus-4-8"));
    assert!(provenance.contains("Responder model | claude-haiku-4-5-20251001"));
    assert!(provenance.contains("Label | canonical-run"));
}

/// The gap this closes: a report could pin the codebase commit while the
/// skill side was "whatever was on disk", which is not a claim anyone can
/// check. The row says which skill revision was measured, and says out loud
/// when uncommitted work means the revision alone does not identify it.
#[test]
fn provenance_names_the_skill_source_and_its_uncommitted_state() {
    let f = fixture(1);
    let mut conditions: Value = serde_json::from_str(CONDITIONS_WITH_PROVENANCE).unwrap();
    conditions["skill_source"] = serde_json::json!({
        "kind": "path",
        "source": f.skill_subdir.to_string_lossy(),
        "resolved_path": f.skill_subdir.to_string_lossy(),
        "revision": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
        "origin_url": "https://example.com/skills.git",
        "branch": "main",
        "host_local": true,
        "dirty": true
    });
    write(
        &f.iteration_dir.join("conditions.json"),
        &serde_json::to_string(&conditions).unwrap(),
    );
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    promote_baseline(&opts(&f, 1)).unwrap();

    let provenance = fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
    assert!(provenance.contains("Skill source"), "{provenance}");
    assert!(provenance.contains("a1b2c3d"), "{provenance}");
    assert!(provenance.contains("uncommitted"), "{provenance}");
    assert!(
        provenance.contains("https://example.com/skills.git"),
        "{provenance}"
    );
}

#[test]
fn provenance_names_every_multi_skill_source() {
    let f = fixture(1);
    let mut conditions: Value = serde_json::from_str(CONDITIONS_WITH_PROVENANCE).unwrap();
    let owner_path = f.skill_subdir.to_string_lossy();
    conditions["skill_source"] = serde_json::json!({
        "kind": "path",
        "source": owner_path,
        "resolved_path": owner_path,
        "branch": "main",
        "host_local": true,
        "dirty": false,
        "eval_owner": "mr-review",
        "skills": [
            {
                "name": "mr-review",
                "kind": "path",
                "source": owner_path,
                "resolved_path": owner_path,
                "revision": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
                "branch": "main",
                "host_local": true,
                "dirty": false
            },
            {
                "name": "review-verification",
                "kind": "path",
                "source": "/skills/review-verification",
                "resolved_path": "/skills/review-verification",
                "revision": "b2c3d4e5f60718293a4b5c6d7e8f90123456789a",
                "branch": "main",
                "host_local": true,
                "dirty": true
            }
        ],
        "siblings": ["ambient-helper"]
    });
    write(
        &f.iteration_dir.join("conditions.json"),
        &serde_json::to_string(&conditions).unwrap(),
    );
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    promote_baseline(&opts(&f, 1)).unwrap();

    let provenance = fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
    assert!(provenance.contains("mr-review:"), "{provenance}");
    assert!(provenance.contains("a1b2c3d"), "{provenance}");
    assert!(provenance.contains("review-verification:"), "{provenance}");
    assert!(provenance.contains("b2c3d4e"), "{provenance}");
    assert!(
        provenance.contains("ambient skills: ambient-helper"),
        "{provenance}"
    );
}

/// The baseline belongs to the skill the *run* measured. Deriving it from the
/// operator's current selection instead would write into whichever skill they
/// happen to be pointing at now.
#[test]
fn the_baseline_follows_the_skill_source_the_run_recorded() {
    let f = fixture(1);
    let recorded = f.skill_subdir.parent().unwrap().join("recorded-skill");
    write(
        &recorded.join("SKILL.md"),
        "---\nname: recorded-skill\n---\n\nbody\n",
    );
    // Only the recorded tree is a repository, so a commit in the provenance table
    // can only have come from it.
    crate::core::run_git(
        &["init", "--quiet", "--initial-branch", "main", "."],
        &recorded,
    );
    crate::core::run_git(&["add", "--all"], &recorded);
    crate::core::run_git(
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@localhost",
            "commit",
            "--quiet",
            "--no-gpg-sign",
            "-m",
            "initial",
        ],
        &recorded,
    );
    let mut conditions: Value = serde_json::from_str(CONDITIONS_WITH_PROVENANCE).unwrap();
    conditions["skill_source"] = serde_json::json!({
        "kind": "path",
        "source": recorded.to_string_lossy(),
        "resolved_path": recorded.to_string_lossy(),
        "branch": "main",
        "host_local": true
    });
    write(
        &f.iteration_dir.join("conditions.json"),
        &serde_json::to_string(&conditions).unwrap(),
    );
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    promote_baseline(&opts(&f, 1)).unwrap();

    assert!(
        recorded.join("evals/baseline/BASELINE.md").exists(),
        "baseline did not follow the recorded skill source"
    );
    // The commit labelling the baseline must come from the repository the baseline
    // landed in, not from wherever the operator happens to be standing.
    let provenance = fs::read_to_string(recorded.join("evals/baseline/BASELINE.md")).unwrap();
    let head = git_head(&recorded);
    assert_ne!(head, "unknown", "fixture did not create a repository");
    assert!(
        provenance.contains(&format!("| Promoted from commit | {head} |")),
        "commit came from the wrong tree; expected {head} in {provenance}"
    );
    assert!(
        !f.skill_subdir.join("evals/baseline").exists(),
        "baseline went to the operator's current selection instead"
    );
}

/// A recorded pointer to a skill that has since moved is a hard failure: a
/// silent fall back to the current selection would write the baseline of one
/// skill into another.
#[test]
fn a_recorded_skill_source_that_no_longer_exists_fails_loudly() {
    let f = fixture(1);
    let mut conditions: Value = serde_json::from_str(CONDITIONS_WITH_PROVENANCE).unwrap();
    conditions["skill_source"] = serde_json::json!({
        "kind": "path",
        "source": "/nowhere/moved-skill",
        "resolved_path": "/nowhere/moved-skill",
        "branch": "main",
        "host_local": true
    });
    write(
        &f.iteration_dir.join("conditions.json"),
        &serde_json::to_string(&conditions).unwrap(),
    );
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    let error = promote_baseline(&opts(&f, 1)).unwrap_err().to_string();

    assert!(error.contains("/nowhere/moved-skill"), "{error}");
    assert!(!f.skill_subdir.join("evals/baseline").exists());
}

/// A published baseline is read by people deciding whether to believe it.
/// Naming the commit is what lets them check.
#[test]
fn provenance_names_the_codebase_and_the_commit_it_resolved_to() {
    let f = fixture(1);
    let conditions: Value = serde_json::from_str(CONDITIONS_WITH_PROVENANCE).unwrap();
    let mut conditions = conditions;
    conditions["codebases"] = serde_json::json!([{
        "kind": "git",
        "source": "https://example.com/project.git",
        "ref": "v1.4.0",
        "revision": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
        "branch": "v1.4.0",
        "evals": ["e1"]
    }]);
    write(
        &f.iteration_dir.join("conditions.json"),
        &serde_json::to_string(&conditions).unwrap(),
    );
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    promote_baseline(&opts(&f, 1)).unwrap();

    let provenance = fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
    assert!(provenance.contains("Codebase"), "{provenance}");
    assert!(
        provenance.contains("https://example.com/project.git"),
        "{provenance}"
    );
    assert!(provenance.contains("v1.4.0"), "{provenance}");
    assert!(provenance.contains("a1b2c3d"), "{provenance}");
}

#[test]
fn provenance_names_when_codebase_skill_sources_were_excluded() {
    let f = fixture(1);
    let mut conditions: Value = serde_json::from_str(CONDITIONS_WITH_PROVENANCE).unwrap();
    conditions["codebases"] = serde_json::json!([{
        "kind": "git",
        "source": "https://example.com/project.git",
        "ref": "v1.4.0",
        "revision": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
        "branch": "v1.4.0",
        "exclude_skill_sources": true,
        "evals": ["e1"]
    }]);
    write(
        &f.iteration_dir.join("conditions.json"),
        &serde_json::to_string(&conditions).unwrap(),
    );
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    promote_baseline(&opts(&f, 1)).unwrap();

    let provenance = fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
    assert!(
        provenance.contains("project skill sources excluded"),
        "{provenance}"
    );
}

/// A host-local path is not reproducible by the reader, so the row says so
/// rather than presenting it like a resolvable reference.
#[test]
fn provenance_marks_a_host_local_codebase_as_unreproducible() {
    let f = fixture(1);
    let mut conditions: Value = serde_json::from_str(CONDITIONS_WITH_PROVENANCE).unwrap();
    conditions["codebases"] = serde_json::json!([{
        "kind": "path",
        "source": "../fixtures/legacy-service",
        "revision": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
        "origin_url": "https://example.com/legacy.git",
        "branch": "main",
        "host_local": true,
        "evals": ["e1"]
    }]);
    write(
        &f.iteration_dir.join("conditions.json"),
        &serde_json::to_string(&conditions).unwrap(),
    );
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    promote_baseline(&opts(&f, 1)).unwrap();

    let provenance = fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
    assert!(provenance.contains("host-local"), "{provenance}");
    // The origin is what a reader elsewhere can actually resolve.
    assert!(
        provenance.contains("https://example.com/legacy.git"),
        "{provenance}"
    );
}

#[test]
fn promote_flags_override_manifest_values() {
    let f = fixture(1);
    write(
        &f.iteration_dir.join("conditions.json"),
        CONDITIONS_WITH_PROVENANCE,
    );
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    let mut o = opts(&f, 1);
    o.agent_model = Some("claude-fable-5");
    o.label = Some("override-label");
    promote_baseline(&o).unwrap();

    let provenance = fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
    assert!(provenance.contains("Agent model | claude-fable-5"));
    // Judge model not overridden — manifest value still wins over "unspecified".
    assert!(provenance.contains("Judge model | claude-opus-4-8"));
    assert!(provenance.contains("Label | override-label"));
}

#[test]
fn writes_notes_stub_when_absent() {
    let f = fixture(2);
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );

    let res = promote_baseline(&opts(&f, 2)).unwrap();

    assert_eq!(res.notes, NotesStatus::StubWritten);
    let notes = fs::read_to_string(res.baseline_dir.join("NOTES.md")).unwrap();
    assert!(notes.contains("mr-review"));
    assert!(notes.contains("iteration-2"));
}

#[test]
fn retains_existing_notes_untouched() {
    let f = fixture(3);
    write(
        &f.iteration_dir.join("benchmark.json"),
        r#"{"delta":{"pass_rate":0}}"#,
    );
    let notes_path = f.skill_subdir.join("evals/baseline/NOTES.md");
    write(
        &notes_path,
        "human-authored observations from iteration-2\n",
    );

    let res = promote_baseline(&opts(&f, 3)).unwrap();

    assert_eq!(res.notes, NotesStatus::RetainedFromPrior);
    assert_eq!(
        fs::read_to_string(&notes_path).unwrap(),
        "human-authored observations from iteration-2\n"
    );
}

#[test]
fn fails_clearly_when_iteration_dir_is_missing() {
    let f = fixture(1); // creates iteration-1, but we promote iteration-9
    let err = promote_baseline(&opts(&f, 9)).unwrap_err();
    assert!(matches!(err, WorkspaceError::Message(_)));
    assert!(err.to_string().contains("iteration-9"));
}
