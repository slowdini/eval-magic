//! Skill snapshotting.
//!
//! Ports the `snapshot` command logic from eval-runner's `cli/run.ts`
//! (`commandSnapshot` / `snapshotFromRef`): capture a skill's `SKILL.md` plus
//! sibling assets into `skills-workspace/<skill>/snapshots/<label>/`, either from
//! the working tree or — read straight from the git object database without
//! touching the working tree (issue #122) — as it existed at a git ref. The
//! `evals/` directory is always excluded; a `.snapshot-meta.json` records the
//! source so `teardown` knows whether the snapshot is reproducible.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::core::run_git;
use crate::workspace::teardown::SNAPSHOT_META;
use crate::workspace::{WorkspaceError, write_json};

/// Snapshot the skill under `skill_subdir` into
/// `<workspace_root>/<skill_name>/snapshots/<label>/`. With `reference` set, the
/// content is read from that git ref; otherwise from the working tree. Errors if
/// a snapshot with this label already exists.
pub fn snapshot(
    workspace_root: &Path,
    skill_name: &str,
    skill_subdir: &Path,
    label: &str,
    reference: Option<&str>,
) -> Result<PathBuf, WorkspaceError> {
    let dest_dir = workspace_root
        .join(skill_name)
        .join("snapshots")
        .join(label);
    if dest_dir.exists() {
        return Err(WorkspaceError::Message(format!(
            "snapshot already exists: {}\n  Use a different --label or delete the existing snapshot first.",
            dest_dir.display()
        )));
    }

    match reference {
        Some(reference) => snapshot_from_ref(reference, skill_subdir, &dest_dir)?,
        None => snapshot_from_working_tree(skill_subdir, &dest_dir)?,
    }
    Ok(dest_dir)
}

/// Copy `SKILL.md` + sibling assets (excluding `evals/`) from the working tree,
/// then record working-tree provenance.
fn snapshot_from_working_tree(skill_subdir: &Path, dest_dir: &Path) -> Result<(), WorkspaceError> {
    let skill_md = skill_subdir.join("SKILL.md");
    if !skill_md.exists() {
        return Err(WorkspaceError::Message(format!(
            "skill not found: {}",
            skill_md.display()
        )));
    }
    fs::create_dir_all(dest_dir)?;
    fs::copy(&skill_md, dest_dir.join("SKILL.md"))?;

    for entry in fs::read_dir(skill_subdir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "SKILL.md" || name == "evals" {
            continue;
        }
        let src = entry.path();
        let dst = dest_dir.join(&name);
        if src.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }

    // Record provenance so teardown keeps this (working-tree) snapshot — unlike a
    // ref snapshot, it can't be regenerated from git.
    write_json(
        &dest_dir.join(SNAPSHOT_META),
        &json!({ "source": "working-tree" }),
    )
}

/// Materialize the skill (`SKILL.md` + sibling assets, excluding `evals/`) as it
/// existed at `reference`, read from the git object database without touching the
/// working tree, then record ref provenance. Git runs from `skill_subdir`, which
/// must sit inside a repo; a bad ref or a skill absent at that ref errors with a
/// clear message.
fn snapshot_from_ref(
    reference: &str,
    skill_subdir: &Path,
    dest_dir: &Path,
) -> Result<(), WorkspaceError> {
    let skill_md = match git_show_bytes(skill_subdir, reference, "SKILL.md") {
        Some(bytes) => bytes,
        None => {
            return Err(WorkspaceError::Message(format!(
                "skill not found at {reference}: {}\n  Check the ref exists and that the skill was present there (and that this is a git repo).",
                skill_subdir.join("SKILL.md").display()
            )));
        }
    };

    fs::create_dir_all(dest_dir)?;
    fs::write(dest_dir.join("SKILL.md"), &skill_md)?;

    for rel in git_ls_tree(skill_subdir, reference)? {
        if rel == "SKILL.md" || rel == "evals" || rel.starts_with("evals/") {
            continue;
        }
        // Listed but unreadable (e.g. submodule/gitlink) — skip.
        let Some(bytes) = git_show_bytes(skill_subdir, reference, &rel) else {
            continue;
        };
        let dst = dest_dir.join(&rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dst, &bytes)?;
    }

    write_json(
        &dest_dir.join(SNAPSHOT_META),
        &json!({ "source": "ref", "ref": reference }),
    )
}

/// `git show <ref>:./<rel_path>` from `cwd`, returning the raw object bytes (so
/// binary assets round-trip), or `None` when the object doesn't exist at that
/// ref. Runs git directly (no shell), so the ref/path aren't interpolated into a
/// shell string.
fn git_show_bytes(cwd: &Path, reference: &str, rel_path: &str) -> Option<Vec<u8>> {
    let spec = format!("{reference}:./{rel_path}");
    let res = run_git(&["show", &spec], cwd);
    (res.status == Some(0)).then_some(res.stdout)
}

/// List every file under `cwd` as it existed at `reference`, as paths relative to
/// `cwd`. Errors with git's stderr on failure — a bad ref or a cwd outside any
/// repo surfaces here.
fn git_ls_tree(cwd: &Path, reference: &str) -> Result<Vec<String>, WorkspaceError> {
    let res = run_git(&["ls-tree", "-r", "--name-only", reference, "."], cwd);
    if res.status != Some(0) {
        return Err(WorkspaceError::Message(format!(
            "git ls-tree failed for ref {reference}: {}",
            String::from_utf8_lossy(&res.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&res.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Recursively copy `src` directory into `dst`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), WorkspaceError> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// Run git with deterministic identity / no signing, asserting success.
    fn git(args: &[&str], cwd: &Path) {
        let mut full = vec![
            "-c",
            "user.email=eval@test",
            "-c",
            "user.name=eval",
            "-c",
            "commit.gpgsign=false",
        ];
        full.extend_from_slice(args);
        let res = run_git(&full, cwd);
        assert_eq!(
            res.status,
            Some(0),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&res.stderr)
        );
    }

    struct Repo {
        _tmp: TempDir,
        skill_subdir: PathBuf,
        workspace_root: PathBuf,
    }

    /// Build a git repo with a `mr-review` skill committed as v1 (plus any
    /// `extra` committed files), then diverge the working-tree SKILL.md to v2.
    fn setup_repo(extra: &[(&str, &str)]) -> Repo {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let skill_subdir = root.join("skill-dir").join("mr-review");
        write(&skill_subdir.join("SKILL.md"), "v1 baseline\n");
        for (rel, content) in extra {
            write(&skill_subdir.join(rel), content);
        }

        git(&["init", "-q"], root);
        git(&["add", "-A"], root);
        git(&["commit", "-q", "-m", "v1"], root);

        // Working tree diverges to v2; the commit still holds v1.
        write(&skill_subdir.join("SKILL.md"), "v2 working tree\n");

        let workspace_root = root.join("work").join("skills-workspace");
        Repo {
            _tmp: tmp,
            skill_subdir,
            workspace_root,
        }
    }

    fn snap_path(repo: &Repo, label: &str, rel: &str) -> PathBuf {
        repo.workspace_root
            .join("mr-review")
            .join("snapshots")
            .join(label)
            .join(rel)
    }

    #[test]
    fn ref_snapshot_reads_committed_skill_md_leaving_working_tree_untouched() {
        let repo = setup_repo(&[]);
        snapshot(
            &repo.workspace_root,
            "mr-review",
            &repo.skill_subdir,
            "old",
            Some("HEAD"),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(snap_path(&repo, "old", "SKILL.md")).unwrap(),
            "v1 baseline\n"
        );
        // Working tree still holds the edited v2 (no clobber).
        assert_eq!(
            fs::read_to_string(repo.skill_subdir.join("SKILL.md")).unwrap(),
            "v2 working tree\n"
        );
    }

    #[test]
    fn ref_snapshot_captures_sibling_assets_but_excludes_evals() {
        let repo = setup_repo(&[
            ("assets/notes.md", "asset body\n"),
            (
                "evals/evals.json",
                "{\"skill_name\":\"mr-review\",\"evals\":[]}",
            ),
        ]);
        snapshot(
            &repo.workspace_root,
            "mr-review",
            &repo.skill_subdir,
            "old",
            Some("HEAD"),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(snap_path(&repo, "old", "assets/notes.md")).unwrap(),
            "asset body\n"
        );
        assert!(!snap_path(&repo, "old", "evals").exists());
    }

    #[test]
    fn ref_snapshot_records_ref_provenance() {
        let repo = setup_repo(&[]);
        snapshot(
            &repo.workspace_root,
            "mr-review",
            &repo.skill_subdir,
            "old",
            Some("HEAD"),
        )
        .unwrap();

        let meta: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(snap_path(&repo, "old", SNAPSHOT_META)).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["source"], "ref");
        assert_eq!(meta["ref"], "HEAD");
    }

    #[test]
    fn ref_that_does_not_exist_fails_with_clear_message() {
        let repo = setup_repo(&[]);
        let err = snapshot(
            &repo.workspace_root,
            "mr-review",
            &repo.skill_subdir,
            "old",
            Some("does-not-exist"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("does-not-exist"));
    }

    #[test]
    fn without_ref_snapshot_reads_the_working_tree_v2() {
        let repo = setup_repo(&[]);
        snapshot(
            &repo.workspace_root,
            "mr-review",
            &repo.skill_subdir,
            "wt",
            None,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(snap_path(&repo, "wt", "SKILL.md")).unwrap(),
            "v2 working tree\n"
        );
    }

    #[test]
    fn working_tree_snapshot_records_working_tree_provenance() {
        let repo = setup_repo(&[]);
        snapshot(
            &repo.workspace_root,
            "mr-review",
            &repo.skill_subdir,
            "wt",
            None,
        )
        .unwrap();
        let meta: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(snap_path(&repo, "wt", SNAPSHOT_META)).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["source"], "working-tree");
    }

    #[test]
    fn duplicate_label_fails() {
        let repo = setup_repo(&[]);
        snapshot(
            &repo.workspace_root,
            "mr-review",
            &repo.skill_subdir,
            "wt",
            None,
        )
        .unwrap();
        let err = snapshot(
            &repo.workspace_root,
            "mr-review",
            &repo.skill_subdir,
            "wt",
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("snapshot already exists"));
    }
}
