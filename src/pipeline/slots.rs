//! Run-slot enumeration for a condition cell.
//!
//! A condition directory holds either a single run directly (the legacy flat
//! layout: `eval-<id>/<cond>/run.json`) or N nested runs under 1-based
//! `run-<k>` subdirectories (`eval-<id>/<cond>/run-2/run.json`). Stages that
//! walk the workspace by convention enumerate slots through [`run_slots`] so
//! both layouts — including a mix across cells within one iteration — read the
//! same way.

use std::fs;
use std::path::{Path, PathBuf};

/// One run's artifact directory within a condition cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSlot {
    /// 1-based index parsed from the `run-<k>` directory name; `None` for the
    /// legacy flat layout where the condition directory is the single slot.
    pub run_index: Option<u32>,
    pub dir: PathBuf,
}

/// The lookup key for a run within the iteration: `<eval_id>:<condition>`, with
/// a `:r<k>` suffix for indexed runs in a multi-run cell.
pub fn run_key(eval_id: &str, condition: &str, run_index: Option<u32>) -> String {
    match run_index {
        Some(k) => format!("{eval_id}:{condition}:r{k}"),
        None => format!("{eval_id}:{condition}"),
    }
}

/// Enumerate the run slots of `cond_dir`: its `run-<k>` subdirectories sorted
/// numerically when any exist, otherwise `cond_dir` itself as the single
/// legacy slot. Existence checks stay with the caller — a nonexistent
/// `cond_dir` still yields the legacy slot.
pub fn run_slots(cond_dir: &Path) -> Vec<RunSlot> {
    let mut slots: Vec<RunSlot> = fs::read_dir(cond_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let index: u32 = name.strip_prefix("run-")?.parse().ok()?;
            Some(RunSlot {
                run_index: Some(index),
                dir: e.path(),
            })
        })
        .collect();

    if slots.is_empty() {
        return vec![RunSlot {
            run_index: None,
            dir: cond_dir.to_path_buf(),
        }];
    }
    slots.sort_by_key(|s| s.run_index);
    slots
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn legacy_slot(cond_dir: &Path) -> Vec<RunSlot> {
        vec![RunSlot {
            run_index: None,
            dir: cond_dir.to_path_buf(),
        }]
    }

    #[test]
    fn flat_cond_dir_yields_single_legacy_slot() {
        let root = TempDir::new().unwrap();
        let cond_dir = root.path().join("with_skill");
        fs::create_dir_all(&cond_dir).unwrap();
        fs::write(cond_dir.join("run.json"), "{}").unwrap();

        assert_eq!(run_slots(&cond_dir), legacy_slot(&cond_dir));
    }

    #[test]
    fn nonexistent_cond_dir_yields_single_legacy_slot() {
        let root = TempDir::new().unwrap();
        let cond_dir = root.path().join("missing");

        assert_eq!(run_slots(&cond_dir), legacy_slot(&cond_dir));
    }

    #[test]
    fn nested_run_dirs_yield_indexed_slots_sorted_numerically() {
        let root = TempDir::new().unwrap();
        let cond_dir = root.path().join("with_skill");
        for k in [10, 2, 1] {
            fs::create_dir_all(cond_dir.join(format!("run-{k}"))).unwrap();
        }

        let slots = run_slots(&cond_dir);
        assert_eq!(
            slots
                .iter()
                .map(|s| s.run_index)
                .collect::<Vec<Option<u32>>>(),
            vec![Some(1), Some(2), Some(10)]
        );
        assert_eq!(slots[2].dir, cond_dir.join("run-10"));
    }

    #[test]
    fn entries_that_are_not_run_dirs_are_ignored() {
        let root = TempDir::new().unwrap();
        let cond_dir = root.path().join("with_skill");
        fs::create_dir_all(cond_dir.join("run-1")).unwrap();
        fs::create_dir_all(cond_dir.join("outputs")).unwrap();
        fs::create_dir_all(cond_dir.join("run-zero")).unwrap();
        fs::write(cond_dir.join("run-2"), "a file, not a dir").unwrap();

        let slots = run_slots(&cond_dir);
        assert_eq!(
            slots,
            vec![RunSlot {
                run_index: Some(1),
                dir: cond_dir.join("run-1"),
            }]
        );
    }

    #[test]
    fn cond_dir_with_only_non_run_entries_is_a_legacy_slot() {
        let root = TempDir::new().unwrap();
        let cond_dir = root.path().join("with_skill");
        fs::create_dir_all(cond_dir.join("outputs")).unwrap();
        fs::write(cond_dir.join("grading.json"), "{}").unwrap();

        assert_eq!(run_slots(&cond_dir), legacy_slot(&cond_dir));
    }
}
