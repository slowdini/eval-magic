//! Measuring a task environment against its Git baseline.
//!
//! Every case here drives real `git`: the measurement is a claim about what
//! Git reports, and a stubbed one would only restate this module's own
//! assumptions.

use super::*;
use std::fs;

/// Invoke git in `root`, failing the test with git's own diagnostic.
fn git(isolated: &IsolatedGit, root: &Path, args: &[&str]) {
    let output = isolated.run(root, args, &[]);
    assert_eq!(
        output.status,
        Some(0),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A task environment as `run` leaves one: a Git repository whose start
/// state is `refs/eval-magic/baseline`, with framework outputs excluded.
///
/// Mirrors the steps of `initialize_task_repository`
/// (`src/cli/run/orchestrate/git.rs`) that a measurement actually depends
/// on, so each test below states only its own mutation.
fn baselined_repo(root: &Path) {
    let isolated = IsolatedGit::new().expect("isolated Git configuration");
    let template = isolated.template_dir().to_string_lossy().into_owned();
    git(
        &isolated,
        root,
        &[
            "init",
            "--quiet",
            "--initial-branch",
            "work",
            "--template",
            &template,
            ".",
        ],
    );
    fs::create_dir_all(root.join(".git/info")).unwrap();
    fs::write(root.join(".git/info/exclude"), "/.eval-magic-outputs/\n").unwrap();
    for (name, value) in [
        ("user.name", "eval-magic"),
        ("user.email", "eval-magic@localhost"),
        ("commit.gpgSign", "false"),
    ] {
        git(&isolated, root, &["config", "--local", name, value]);
    }
    git(&isolated, root, &["add", "--all", "--", "."]);
    // What the runner places is forced in on top of the codebase's ignore
    // rules, exactly as `runner_placed_paths` does.
    if root.join(".claude").exists() {
        git(&isolated, root, &["add", "--force", "--", ".claude"]);
    }
    git(
        &isolated,
        root,
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "baseline",
        ],
    );
    git(&isolated, root, &["update-ref", BASELINE_REF, "HEAD"]);
}

#[test]
fn lines_changed_saturates_untrusted_artifact_totals() {
    let metrics = DiffScopeMetrics {
        lines_added: u64::MAX,
        lines_removed: 1,
        ..DiffScopeMetrics::default()
    };
    assert_eq!(metrics.lines_changed(), u64::MAX);
}

#[test]
fn measurement_counts_all_task_changes_except_framework_outputs() {
    let temp = tempfile::TempDir::new().unwrap();
    let eval_root = temp.path().join("env");
    let run_dir = temp.path().join("run");
    let outputs_dir = eval_root.join(".eval-magic-outputs/eval-e1/with_skill");
    fs::create_dir_all(&run_dir).unwrap();
    fs::create_dir_all(eval_root.join("src")).unwrap();
    fs::create_dir_all(&outputs_dir).unwrap();
    fs::write(eval_root.join("src/changed.txt"), "old\nsame\n").unwrap();
    fs::write(eval_root.join("src/deleted.txt"), "gone\n").unwrap();
    fs::write(eval_root.join("framework.txt"), "before\n").unwrap();

    baselined_repo(&eval_root);

    fs::write(eval_root.join("src/changed.txt"), "new\nsame\n").unwrap();
    fs::remove_file(eval_root.join("src/deleted.txt")).unwrap();
    fs::write(eval_root.join("framework.txt"), "after\n").unwrap();
    fs::write(eval_root.join("notes.txt"), "one\ntwo\n").unwrap();
    fs::write(outputs_dir.join("final-message.md"), "ignored\n").unwrap();
    fs::write(
        eval_root.join(".eval-magic-outputs/agent-created.txt"),
        "also ignored\n",
    )
    .unwrap();

    let record = measure_task_diff(&eval_root, &run_dir).unwrap();
    assert_eq!(
        record.metrics,
        DiffScopeMetrics {
            files_touched: 4,
            lines_added: 4,
            lines_removed: 3,
            hunks: 4,
        }
    );
}

/// Git refuses to index any path with a `.git` component, so a nested
/// repository's internals are invisible to a measurement — not just the
/// runner-owned root `.git`.
#[test]
fn measurement_ignores_every_git_directory_not_just_the_runner_owned_root() {
    let temp = tempfile::TempDir::new().unwrap();
    let eval_root = temp.path().join("env");
    let run_dir = temp.path().join("run");
    fs::create_dir_all(&run_dir).unwrap();
    fs::create_dir_all(eval_root.join("vendor/.git")).unwrap();
    fs::write(eval_root.join("vendor/.git/config"), "nested-before\n").unwrap();
    fs::write(eval_root.join("source.txt"), "before\n").unwrap();

    baselined_repo(&eval_root);

    fs::write(eval_root.join(".git/config-probe"), "root-after\n").unwrap();
    fs::write(eval_root.join("vendor/.git/config"), "nested-after\n").unwrap();
    fs::write(eval_root.join("source.txt"), "after\n").unwrap();

    assert_eq!(
        measure_task_diff(&eval_root, &run_dir).unwrap().metrics,
        DiffScopeMetrics {
            files_touched: 1,
            lines_added: 1,
            lines_removed: 1,
            hunks: 1,
        }
    );
}

/// An iteration holding one dispatched task against `eval_root`, complete
/// enough for `measure_iteration_diff_scopes` to reach the measurement.
fn iteration_with_one_task(iteration_dir: &Path, eval_root: &Path) -> std::path::PathBuf {
    let run_dir = iteration_dir.join("eval-e1/with_skill");
    fs::create_dir_all(&run_dir).unwrap();
    let run_record_path = run_dir.join("run.json");
    fs::write(&run_record_path, "{}").unwrap();
    fs::write(
        iteration_dir.join("dispatch.json"),
        serde_json::json!({
            "tasks": [{
                "eval_id": "e1",
                "condition": "with_skill",
                "eval_root": eval_root.to_string_lossy(),
                "run_record_path": run_record_path.to_string_lossy(),
            }],
        })
        .to_string(),
    )
    .unwrap();
    run_dir
}

#[test]
fn a_baselined_environment_is_measured_from_its_ref() {
    let temp = tempfile::TempDir::new().unwrap();
    let eval_root = temp.path().join("env");
    let iteration_dir = temp.path().join("iteration-1");
    fs::create_dir_all(&eval_root).unwrap();
    fs::create_dir_all(&iteration_dir).unwrap();
    fs::write(eval_root.join("source.txt"), "before\n").unwrap();

    baselined_repo(&eval_root);
    let run_dir = iteration_with_one_task(&iteration_dir, &eval_root);
    fs::write(eval_root.join("source.txt"), "after\n").unwrap();

    let summary = measure_iteration_diff_scopes(&iteration_dir).unwrap();
    assert_eq!(summary.measured, 1, "{summary:?}");
    assert_eq!(summary.missing_baseline, 0, "{summary:?}");
    assert_eq!(
        serde_json::from_str::<DiffScopeMetrics>(
            &fs::read_to_string(run_dir.join(RESULT_FILE)).unwrap()
        )
        .unwrap(),
        DiffScopeMetrics {
            files_touched: 1,
            lines_added: 1,
            lines_removed: 1,
            hunks: 1,
        }
    );
}

/// A torn-down environment, or one from an iteration built before the
/// baseline ref existed, has nothing to measure against. That is a reported
/// gap, not a failure of the whole stage.
#[test]
fn an_environment_without_a_baseline_ref_is_reported_as_unmeasurable() {
    let temp = tempfile::TempDir::new().unwrap();
    let eval_root = temp.path().join("env");
    let iteration_dir = temp.path().join("iteration-1");
    fs::create_dir_all(&eval_root).unwrap();
    fs::create_dir_all(&iteration_dir).unwrap();
    let run_dir = iteration_with_one_task(&iteration_dir, &eval_root);

    let summary = measure_iteration_diff_scopes(&iteration_dir).unwrap();
    assert_eq!(summary.missing_baseline, 1, "{summary:?}");
    assert_eq!(summary.measured, 0, "{summary:?}");
    assert!(
        summary.warnings[0].contains("e1/with_skill"),
        "{:?}",
        summary.warnings
    );
    assert!(
        !run_dir.join(RESULT_FILE).exists(),
        "an unmeasurable task must not freeze a result"
    );
}

/// A changed environment yields a patch beside its metrics — the evidence a
/// judge needs, which the counters alone cannot carry.
#[test]
fn a_measurement_writes_the_patch_beside_its_metrics() {
    let temp = tempfile::TempDir::new().unwrap();
    let eval_root = temp.path().join("env");
    let run_dir = temp.path().join("run");
    fs::create_dir_all(&eval_root).unwrap();
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(eval_root.join("source.txt"), "before\n").unwrap();

    baselined_repo(&eval_root);
    fs::write(eval_root.join("source.txt"), "after\n").unwrap();

    let record = measure_task_diff(&eval_root, &run_dir).unwrap();
    let patch = fs::read_to_string(run_dir.join(PATCH_FILE)).unwrap();
    assert!(patch.contains("--- a/source.txt"), "{patch}");
    assert!(patch.contains("-before"), "{patch}");
    assert!(patch.contains("+after"), "{patch}");
    assert!(!record.patch.truncated, "{record:?}");
    assert_eq!(record.patch.bytes, patch.len() as u64);
    assert_eq!(record.patch.path, PATCH_FILE);
}

/// An agent that changed nothing is a real, reportable outcome: zero
/// metrics and a patch that exists and is empty, never a missing artifact.
#[test]
fn a_run_with_no_changes_reports_zero_metrics_and_an_empty_patch() {
    let temp = tempfile::TempDir::new().unwrap();
    let eval_root = temp.path().join("env");
    let run_dir = temp.path().join("run");
    fs::create_dir_all(&eval_root).unwrap();
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(eval_root.join("source.txt"), "untouched\n").unwrap();

    baselined_repo(&eval_root);

    let record = measure_task_diff(&eval_root, &run_dir).unwrap();
    assert_eq!(record.metrics, DiffScopeMetrics::default());
    assert_eq!(record.patch.bytes, 0);
    assert!(!record.patch.truncated);
    assert_eq!(fs::read_to_string(run_dir.join(PATCH_FILE)).unwrap(), "");
}

/// The counters say how much changed; this says what. A judge reading the
/// record can see the shape of the work before opening the patch.
#[test]
fn the_record_lists_every_changed_file_with_its_status() {
    let temp = tempfile::TempDir::new().unwrap();
    let eval_root = temp.path().join("env");
    let run_dir = temp.path().join("run");
    fs::create_dir_all(&eval_root).unwrap();
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(eval_root.join("kept.txt"), "steady\n").unwrap();
    fs::write(eval_root.join("changed.txt"), "old\n").unwrap();
    fs::write(eval_root.join("removed.txt"), "gone\n").unwrap();

    baselined_repo(&eval_root);

    fs::write(eval_root.join("changed.txt"), "new\n").unwrap();
    fs::remove_file(eval_root.join("removed.txt")).unwrap();
    fs::write(eval_root.join("created.txt"), "fresh\nlines\n").unwrap();

    let record = measure_task_diff(&eval_root, &run_dir).unwrap();
    assert_eq!(
        record.files,
        vec![
            ChangedFile {
                path: "changed.txt".to_string(),
                status: ChangeStatus::Modified,
                lines_added: 1,
                lines_removed: 1,
            },
            ChangedFile {
                path: "created.txt".to_string(),
                status: ChangeStatus::Added,
                lines_added: 2,
                lines_removed: 0,
            },
            ChangedFile {
                path: "removed.txt".to_string(),
                status: ChangeStatus::Deleted,
                lines_added: 0,
                lines_removed: 1,
            },
        ]
    );
}

#[test]
fn a_patch_within_the_cap_is_written_whole() {
    let (kept, truncated) = truncate_patch(b"one\ntwo\n", 64);
    assert_eq!(kept, b"one\ntwo\n");
    assert!(!truncated);
}

#[test]
fn a_patch_past_the_cap_keeps_whole_lines_and_says_it_was_cut() {
    let (kept, truncated) = truncate_patch(b"aaaa\nbbbb\ncccc\n", 12);
    assert!(truncated);
    let text = String::from_utf8(kept).unwrap();
    assert!(text.starts_with("aaaa\nbbbb\n"), "{text}");
    assert!(!text.contains("cccc"), "{text}");
    assert!(text.contains("truncated"), "{text}");
    assert!(text.ends_with('\n'), "{text}");
}

/// One line longer than the whole cap has no boundary to cut on. Capping
/// still wins — an uncapped artifact is the thing being prevented.
#[test]
fn a_patch_with_no_line_boundary_inside_the_cap_is_still_cut() {
    let (kept, truncated) = truncate_patch(b"aaaaaaaaaaaaaaaaaaaa\n", 8);
    assert!(truncated);
    let text = String::from_utf8(kept).unwrap();
    assert!(text.starts_with("aaaaaaaa"), "{text}");
    assert!(text.contains("truncated"), "{text}");
}

/// Capping the evidence must not cap the measurement: the counters describe
/// the whole diff even when the patch beside them stops early.
#[test]
fn a_diff_past_the_cap_is_captured_truncated_while_the_metrics_stay_whole() {
    let temp = tempfile::TempDir::new().unwrap();
    let eval_root = temp.path().join("env");
    let run_dir = temp.path().join("run");
    fs::create_dir_all(&eval_root).unwrap();
    fs::create_dir_all(&run_dir).unwrap();

    baselined_repo(&eval_root);

    let lines = 200_000;
    let bulk: String = (0..lines)
        .map(|n| format!("generated line {n}\n"))
        .collect();
    assert!(
        bulk.len() > PATCH_BYTE_LIMIT,
        "the fixture must exceed the cap"
    );
    fs::write(eval_root.join("generated.txt"), &bulk).unwrap();

    let record = measure_task_diff(&eval_root, &run_dir).unwrap();
    assert!(record.patch.truncated, "{:?}", record.patch);
    assert_eq!(record.metrics.lines_added, lines);
    assert_eq!(record.metrics.files_touched, 1);

    let patch = fs::read(run_dir.join(PATCH_FILE)).unwrap();
    assert_eq!(record.patch.bytes, patch.len() as u64);
    assert!(
        patch.len() < bulk.len(),
        "a capped patch must be smaller than the diff it stands for"
    );
    let text = String::from_utf8_lossy(&patch);
    assert!(
        text.trim_end().ends_with("is not captured"),
        "{}",
        &text[text.len() - 200..]
    );
}

/// A real repository ignores its build output, and the baseline commit was
/// built under those same rules — so a run that compiles does not report
/// thousands of touched files. What the runner force-added is tracked
/// despite the rules, and stays measured.
#[test]
fn ignored_files_do_not_count_but_a_force_added_path_still_does() {
    let temp = tempfile::TempDir::new().unwrap();
    let eval_root = temp.path().join("env");
    let run_dir = temp.path().join("run");
    fs::create_dir_all(eval_root.join(".claude/skills")).unwrap();
    fs::create_dir_all(eval_root.join("src")).unwrap();
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(eval_root.join(".gitignore"), "/build/\n/.claude/\n").unwrap();
    fs::write(eval_root.join(".claude/skills/SKILL.md"), "staged\n").unwrap();
    fs::write(eval_root.join("src/main.rs"), "fn main() {}\n").unwrap();

    baselined_repo(&eval_root);

    fs::create_dir_all(eval_root.join("build")).unwrap();
    fs::write(eval_root.join("build/out.o"), "compiled\n").unwrap();
    fs::write(eval_root.join(".claude/skills/SKILL.md"), "edited\n").unwrap();

    let record = measure_task_diff(&eval_root, &run_dir).unwrap();
    assert_eq!(
        record
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec![".claude/skills/SKILL.md"],
        "{:?}",
        record.files
    );
    assert_eq!(record.metrics.files_touched, 1);
}

/// Git detects renames by default, and would report one entry with no line
/// changes. A rename is two touched files — one created and one deleted —
/// which is what the metric has always meant.
#[test]
fn a_rename_counts_as_the_two_files_it_touches() {
    let temp = tempfile::TempDir::new().unwrap();
    let eval_root = temp.path().join("env");
    let run_dir = temp.path().join("run");
    fs::create_dir_all(&eval_root).unwrap();
    fs::create_dir_all(&run_dir).unwrap();
    let body = "alpha\nbeta\ngamma\ndelta\n";
    fs::write(eval_root.join("original.txt"), body).unwrap();

    baselined_repo(&eval_root);

    fs::remove_file(eval_root.join("original.txt")).unwrap();
    fs::write(eval_root.join("moved.txt"), body).unwrap();

    let record = measure_task_diff(&eval_root, &run_dir).unwrap();
    assert_eq!(
        record.files,
        vec![
            ChangedFile {
                path: "moved.txt".to_string(),
                status: ChangeStatus::Added,
                lines_added: 4,
                lines_removed: 0,
            },
            ChangedFile {
                path: "original.txt".to_string(),
                status: ChangeStatus::Deleted,
                lines_added: 0,
                lines_removed: 4,
            },
        ]
    );
    assert_eq!(record.metrics.files_touched, 2);
}
