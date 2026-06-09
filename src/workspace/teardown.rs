//! End-of-run workspace cleanup.
//!
//! Ports `src/workspace/workspace-teardown.ts`: reclaim a skill's ephemeral
//! `skills-workspace/<skill>/` subtree without ever destroying results the user
//! hasn't moved into version control.

use std::fs;
use std::path::Path;

/// Marker `promote-baseline` drops into an iteration dir once that iteration's
/// durable results (benchmark + gradings) are committed under the skill's
/// `evals/baseline/`. Teardown treats its presence as "safe to delete".
pub const PROMOTED_MARKER: &str = ".promoted.json";

/// Provenance the `snapshot` command writes into each `snapshots/<label>/` dir,
/// recording whether it was materialized from a git ref (reproducible) or copied
/// from the working tree (not reproducible). Teardown only reclaims ref snapshots.
pub const SNAPSHOT_META: &str = ".snapshot-meta.json";

/// An iteration kept during cleanup because it still holds uncommitted results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptIteration {
    pub iteration: String,
    pub reason: String,
}

/// What [`cleanup_workspace`] removed and kept.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WorkspaceCleanupSummary {
    /// Iteration dir names removed (promoted, or pure scaffolding).
    pub removed_iterations: Vec<String>,
    /// Iterations kept because they hold uncommitted results, with the reason.
    pub kept_iterations: Vec<KeptIteration>,
    /// Snapshot labels removed (reproducible from a git ref).
    pub removed_snapshots: Vec<String>,
    /// Snapshot labels kept (working-tree or legacy, can't be regenerated).
    pub kept_snapshots: Vec<String>,
    /// True when the skill's whole workspace subtree was removed.
    pub workspace_removed: bool,
}

/// The reason string attached to a kept, unpromoted iteration.
const UNCOMMITTED_REASON: &str = "uncommitted results — not promoted to evals/baseline/";

/// End-of-run cleanup of a skill's `skills-workspace/<skill>/` subtree.
///
/// Per iteration: promoted (marker present) → removed; unpromoted but holding
/// captured results → kept and reported; unpromoted scaffolding → removed. Per
/// snapshot: ref-sourced → removed; working-tree or legacy → kept. Empty parents
/// (`snapshots/`, the skill dir, the workspace root) are pruned, but a non-empty
/// one — e.g. another skill's artifacts — is never touched.
pub fn cleanup_workspace(workspace_root: &Path, skill_name: &str) -> WorkspaceCleanupSummary {
    let mut summary = WorkspaceCleanupSummary::default();

    let skill_dir = workspace_root.join(skill_name);
    if !skill_dir.exists() {
        return summary;
    }

    for name in sorted_entry_names(&skill_dir) {
        if !name.starts_with("iteration-") {
            continue;
        }
        let iter_dir = skill_dir.join(&name);
        if !iter_dir.is_dir() {
            continue;
        }
        if iter_dir.join(PROMOTED_MARKER).exists() {
            let _ = fs::remove_dir_all(&iter_dir);
            summary.removed_iterations.push(name);
        } else if iteration_has_results(&iter_dir) {
            summary.kept_iterations.push(KeptIteration {
                iteration: name,
                reason: UNCOMMITTED_REASON.to_string(),
            });
        } else {
            let _ = fs::remove_dir_all(&iter_dir);
            summary.removed_iterations.push(name);
        }
    }

    let snapshots_dir = skill_dir.join("snapshots");
    if snapshots_dir.exists() {
        for name in sorted_entry_names(&snapshots_dir) {
            let snap_dir = snapshots_dir.join(&name);
            if !snap_dir.is_dir() {
                continue;
            }
            if snapshot_source(&snap_dir).as_deref() == Some("ref") {
                let _ = fs::remove_dir_all(&snap_dir);
                summary.removed_snapshots.push(name);
            } else {
                summary.kept_snapshots.push(name);
            }
        }
        prune_if_empty(&snapshots_dir);
    }

    prune_if_empty(&skill_dir);
    summary.workspace_removed = !skill_dir.exists();
    prune_if_empty(workspace_root);

    summary
}

/// Directory entry names, sorted, so summary vectors are deterministic
/// (`fs::read_dir` order is unspecified). Missing/unreadable dirs yield `[]`.
fn sorted_entry_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// Remove `dir` only if it exists and is empty.
fn prune_if_empty(dir: &Path) {
    if let Ok(mut entries) = fs::read_dir(dir)
        && entries.next().is_none()
    {
        let _ = fs::remove_dir(dir);
    }
}

/// An iteration carries "captured results" worth preserving if it reached the
/// point of producing an aggregate (`benchmark.json`) or any per-run record or
/// grading. Anything short of that is reproducible scaffolding.
fn iteration_has_results(iter_dir: &Path) -> bool {
    if iter_dir.join("benchmark.json").exists() {
        return true;
    }
    let Ok(entries) = fs::read_dir(iter_dir) else {
        return false;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("eval-") {
            continue;
        }
        let eval_dir = entry.path();
        if !eval_dir.is_dir() {
            continue;
        }
        let Ok(conds) = fs::read_dir(&eval_dir) else {
            continue;
        };
        for cond in conds.filter_map(Result::ok) {
            let cond_dir = cond.path();
            if !cond_dir.is_dir() {
                continue;
            }
            if cond_dir.join("run.json").exists() || cond_dir.join("grading.json").exists() {
                return true;
            }
        }
    }
    false
}

/// Read the `source` field from a snapshot's `.snapshot-meta.json`, or `None`
/// when the file is absent or unparseable (legacy snapshots).
fn snapshot_source(snap_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(snap_dir.join(SNAPSHOT_META)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("source")
        .and_then(|s| s.as_str())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Write `body` to `path`, creating parent dirs.
    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[derive(Default)]
    struct IterOpts {
        promoted: bool,
        benchmark: bool,
        run_record: bool,
        grading: bool,
        scaffolding_only: bool,
    }

    fn make_iteration(ws: &Path, skill: &str, iteration: &str, opts: IterOpts) -> PathBuf {
        let dir = ws.join(skill).join(iteration);
        fs::create_dir_all(&dir).unwrap();
        if opts.scaffolding_only {
            write(&dir.join("dispatch.json"), "[]\n");
        }
        if opts.benchmark {
            write(
                &dir.join("benchmark.json"),
                "{\"delta\":{\"pass_rate\":0.5}}\n",
            );
        }
        if opts.run_record {
            write(
                &dir.join("eval-e1/with_skill/run.json"),
                "{\"eval_id\":\"e1\"}\n",
            );
        }
        if opts.grading {
            write(
                &dir.join("eval-e1/with_skill/grading.json"),
                "{\"summary\":{\"pass_rate\":1}}\n",
            );
        }
        if opts.promoted {
            write(&dir.join(PROMOTED_MARKER), "{\"commit\":\"abc1234\"}\n");
        }
        dir
    }

    fn make_snapshot(ws: &Path, skill: &str, label: &str, source: Option<&str>) -> PathBuf {
        let dir = ws.join(skill).join("snapshots").join(label);
        fs::create_dir_all(&dir).unwrap();
        write(&dir.join("SKILL.md"), "snapshot body\n");
        match source {
            Some("ref") => write(
                &dir.join(SNAPSHOT_META),
                "{\"source\":\"ref\",\"ref\":\"HEAD~1\"}\n",
            ),
            Some(s) => write(
                &dir.join(SNAPSHOT_META),
                &format!("{{\"source\":\"{s}\"}}\n"),
            ),
            None => {}
        }
        dir
    }

    fn kept_names(s: &WorkspaceCleanupSummary) -> Vec<String> {
        s.kept_iterations
            .iter()
            .map(|k| k.iteration.clone())
            .collect()
    }

    #[test]
    fn removes_promoted_iteration_and_prunes_workspace() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("skills-workspace");
        fs::create_dir_all(&ws).unwrap();
        let iter = make_iteration(
            &ws,
            "mr-review",
            "iteration-1",
            IterOpts {
                promoted: true,
                benchmark: true,
                grading: true,
                ..Default::default()
            },
        );

        let summary = cleanup_workspace(&ws, "mr-review");

        assert!(!iter.exists());
        assert_eq!(summary.removed_iterations, vec!["iteration-1"]);
        assert!(summary.workspace_removed);
        assert!(!ws.join("mr-review").exists());
        assert!(!ws.exists());
    }

    #[test]
    fn keeps_unpromoted_iteration_with_benchmark_and_reports_it() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("skills-workspace");
        fs::create_dir_all(&ws).unwrap();
        let iter = make_iteration(
            &ws,
            "mr-review",
            "iteration-1",
            IterOpts {
                benchmark: true,
                ..Default::default()
            },
        );

        let summary = cleanup_workspace(&ws, "mr-review");

        assert!(iter.exists());
        assert_eq!(summary.removed_iterations, Vec::<String>::new());
        assert_eq!(kept_names(&summary), vec!["iteration-1"]);
        assert!(ws.exists());
    }

    #[test]
    fn keeps_unpromoted_iteration_with_only_a_run_record() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("skills-workspace");
        fs::create_dir_all(&ws).unwrap();
        let iter = make_iteration(
            &ws,
            "mr-review",
            "iteration-1",
            IterOpts {
                run_record: true,
                ..Default::default()
            },
        );

        let summary = cleanup_workspace(&ws, "mr-review");

        assert!(iter.exists());
        assert_eq!(kept_names(&summary), vec!["iteration-1"]);
    }

    #[test]
    fn removes_unpromoted_scaffolding_only_iteration() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("skills-workspace");
        fs::create_dir_all(&ws).unwrap();
        let iter = make_iteration(
            &ws,
            "mr-review",
            "iteration-1",
            IterOpts {
                scaffolding_only: true,
                ..Default::default()
            },
        );

        let summary = cleanup_workspace(&ws, "mr-review");

        assert!(!iter.exists());
        assert_eq!(summary.removed_iterations, vec!["iteration-1"]);
    }

    #[test]
    fn mixed_promoted_removed_kept_with_results_skill_dir_not_pruned() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("skills-workspace");
        fs::create_dir_all(&ws).unwrap();
        let promoted = make_iteration(
            &ws,
            "mr-review",
            "iteration-1",
            IterOpts {
                promoted: true,
                benchmark: true,
                ..Default::default()
            },
        );
        let kept = make_iteration(
            &ws,
            "mr-review",
            "iteration-2",
            IterOpts {
                benchmark: true,
                ..Default::default()
            },
        );

        let summary = cleanup_workspace(&ws, "mr-review");

        assert!(!promoted.exists());
        assert!(kept.exists());
        assert_eq!(summary.removed_iterations, vec!["iteration-1"]);
        assert_eq!(kept_names(&summary), vec!["iteration-2"]);
        assert!(!summary.workspace_removed);
        assert!(ws.join("mr-review").exists());
    }

    #[test]
    fn removes_ref_snapshots_keeps_working_tree_and_legacy() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("skills-workspace");
        fs::create_dir_all(&ws).unwrap();
        let ref_snap = make_snapshot(&ws, "mr-review", "old-ref", Some("ref"));
        let wt_snap = make_snapshot(&ws, "mr-review", "wt", Some("working-tree"));
        let legacy_snap = make_snapshot(&ws, "mr-review", "legacy", None);

        let summary = cleanup_workspace(&ws, "mr-review");

        assert!(!ref_snap.exists());
        assert!(wt_snap.exists());
        assert!(legacy_snap.exists());
        assert_eq!(summary.removed_snapshots, vec!["old-ref"]);
        assert_eq!(summary.kept_snapshots, vec!["legacy", "wt"]);
    }

    #[test]
    fn never_touches_another_skills_workspace_and_leaves_root_intact() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("skills-workspace");
        fs::create_dir_all(&ws).unwrap();
        make_iteration(
            &ws,
            "mr-review",
            "iteration-1",
            IterOpts {
                promoted: true,
                ..Default::default()
            },
        );
        let other_iter = make_iteration(
            &ws,
            "other-skill",
            "iteration-1",
            IterOpts {
                benchmark: true,
                ..Default::default()
            },
        );

        cleanup_workspace(&ws, "mr-review");

        assert!(!ws.join("mr-review").exists());
        assert!(other_iter.exists());
        assert!(ws.exists());
    }

    #[test]
    fn empty_summary_when_skill_has_no_workspace() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("skills-workspace");
        fs::create_dir_all(&ws).unwrap();

        let summary = cleanup_workspace(&ws, "never-ran");

        assert_eq!(summary, WorkspaceCleanupSummary::default());
    }
}
