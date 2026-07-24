//! Baseline capture and deterministic final-environment diff metrics.
//!
//! A baseline snapshots every file in a task's private `eval_root` after
//! framework setup. Measurement compares the completed task with that snapshot,
//! excluding only the task's `.eval-magic-outputs` subtree.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffTag, capture_diff_slices};
use walkdir::{DirEntry, WalkDir};

use crate::pipeline::error::PipelineError;
use crate::pipeline::io::write_json;

const BASELINE_DIR: &str = "diff-scope-baseline";
const BASELINE_MANIFEST: &str = "manifest.json";
const BASELINE_FILES: &str = "files";
const RESULT_FILE: &str = "diff-scope.json";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffScopeMetrics {
    pub files_touched: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub hunks: u64,
}

impl DiffScopeMetrics {
    pub fn lines_changed(self) -> u64 {
        self.lines_added.saturating_add(self.lines_removed)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct BaselineManifest {
    preexisting_files: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DispatchFile {
    #[serde(default)]
    tasks: Vec<DispatchTask>,
}

#[derive(Debug, Deserialize)]
struct DispatchTask {
    eval_id: String,
    condition: String,
    #[serde(default)]
    run_index: Option<u32>,
    eval_root: Option<String>,
    run_record_path: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DiffScopeSummary {
    pub measured: usize,
    pub reused: usize,
    pub missing_baseline: usize,
    pub shared_environment: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct FileDiff {
    lines_added: u64,
    lines_removed: u64,
    hunks: u64,
}

fn diff_bytes(old: &[u8], new: &[u8]) -> FileDiff {
    let old_lines: Vec<&[u8]> = old.split_inclusive(|byte| *byte == b'\n').collect();
    let new_lines: Vec<&[u8]> = new.split_inclusive(|byte| *byte == b'\n').collect();
    let mut result = FileDiff::default();
    let mut in_hunk = false;

    for operation in capture_diff_slices(Algorithm::Myers, &old_lines, &new_lines) {
        let (tag, old_range, new_range) = operation.as_tag_tuple();
        match tag {
            DiffTag::Equal => in_hunk = false,
            DiffTag::Delete => {
                if !in_hunk {
                    result.hunks += 1;
                    in_hunk = true;
                }
                result.lines_removed += old_range.len() as u64;
            }
            DiffTag::Insert => {
                if !in_hunk {
                    result.hunks += 1;
                    in_hunk = true;
                }
                result.lines_added += new_range.len() as u64;
            }
            DiffTag::Replace => {
                if !in_hunk {
                    result.hunks += 1;
                    in_hunk = true;
                }
                result.lines_removed += old_range.len() as u64;
                result.lines_added += new_range.len() as u64;
            }
        }
    }
    result
}

pub fn capture_iteration_baselines(iteration_dir: &Path) -> Result<(), PipelineError> {
    let dispatch_path = iteration_dir.join("dispatch.json");
    let dispatch: DispatchFile = serde_json::from_str(&fs::read_to_string(&dispatch_path)?)?;
    for task in dispatch.tasks {
        let eval_root = task.eval_root.ok_or_else(|| {
            PipelineError::Message(format!(
                "dispatch task in {} has no eval_root for diff-scope capture",
                dispatch_path.display()
            ))
        })?;
        let run_dir = Path::new(&task.run_record_path).parent().ok_or_else(|| {
            PipelineError::Message(format!(
                "diff-scope task has no run directory in run_record_path: {}",
                task.run_record_path
            ))
        })?;
        fs::create_dir_all(run_dir)?;
        capture_task_baseline(Path::new(&eval_root), run_dir)?;
    }
    Ok(())
}

pub fn measure_iteration_diff_scopes(
    iteration_dir: &Path,
) -> Result<DiffScopeSummary, PipelineError> {
    let dispatch_path = iteration_dir.join("dispatch.json");
    // Legacy and hand-authored iterations may contain run records without a
    // dispatch manifest. They remain gradeable; only an explicit diff_scope
    // assertion makes the missing measurement a finalize-time error.
    if !dispatch_path.exists() {
        return Ok(DiffScopeSummary::default());
    }
    let dispatch: DispatchFile = serde_json::from_str(&fs::read_to_string(&dispatch_path)?)?;
    let root_counts: HashMap<String, usize> = dispatch
        .tasks
        .iter()
        .filter_map(|task| task.eval_root.as_deref())
        .fold(HashMap::new(), |mut counts, root| {
            *counts.entry(root.to_string()).or_default() += 1;
            counts
        });
    let mut summary = DiffScopeSummary::default();

    for task in dispatch.tasks {
        let run_record_path = Path::new(&task.run_record_path);
        let run_dir = run_record_path.parent().ok_or_else(|| {
            PipelineError::Message(format!(
                "diff-scope task has no run directory in run_record_path: {}",
                task.run_record_path
            ))
        })?;
        if !run_record_path.exists() {
            continue;
        }
        let result_path = run_dir.join(RESULT_FILE);
        if result_path.exists() {
            let value = serde_json::from_str(&fs::read_to_string(&result_path)?)?;
            crate::validation::validate_against_schema::<DiffScopeMetrics>(
                crate::validation::SchemaName::DiffScope,
                &value,
                &result_path.to_string_lossy(),
            )?;
            summary.reused += 1;
            continue;
        }
        let run_label = task
            .run_index
            .map(|run| format!("/run-{run}"))
            .unwrap_or_default();
        let Some(eval_root) = task.eval_root.as_deref() else {
            eprintln!(
                "warn: {}/{}{run_label} has no eval_root — diff-scope unavailable; rebuild the iteration to capture metrics",
                task.eval_id, task.condition
            );
            summary.missing_baseline += 1;
            continue;
        };
        if root_counts.get(eval_root).copied().unwrap_or_default() != 1 {
            eprintln!(
                "warn: {}/{}{run_label} shares eval_root with another task — diff-scope unavailable; rebuild the iteration for task-scoped environments",
                task.eval_id, task.condition
            );
            summary.shared_environment += 1;
            continue;
        }
        if !run_dir.join(BASELINE_DIR).join(BASELINE_MANIFEST).exists() {
            eprintln!(
                "warn: {}/{}{run_label} has no pre-dispatch baseline — diff-scope unavailable; rebuild the iteration to capture metrics",
                task.eval_id, task.condition
            );
            summary.missing_baseline += 1;
            continue;
        }
        if run_dir.join("command-checks").exists() {
            return Err(PipelineError::Message(format!(
                "cannot capture diff scope for {}/{}{run_label} after command checks have run; rebuild the iteration",
                task.eval_id, task.condition
            )));
        }

        let metrics = measure_task_diff(Path::new(eval_root), run_dir)?;
        crate::validation::validate_against_schema::<DiffScopeMetrics>(
            crate::validation::SchemaName::DiffScope,
            &serde_json::to_value(metrics)?,
            &result_path.to_string_lossy(),
        )?;
        write_json(&result_path, &metrics)?;
        summary.measured += 1;
    }
    Ok(summary)
}

fn capture_task_baseline(eval_root: &Path, run_dir: &Path) -> Result<(), PipelineError> {
    let baseline_dir = run_dir.join(BASELINE_DIR);
    if baseline_dir.exists() {
        fs::remove_dir_all(&baseline_dir)?;
    }
    let file_snapshot = baseline_dir.join(BASELINE_FILES);
    fs::create_dir_all(&file_snapshot)?;

    let framework_outputs = eval_root.join(".eval-magic-outputs");
    let mut preexisting_paths = walk_files(eval_root, Some(&framework_outputs))?;
    preexisting_paths.sort();
    let preexisting_files = preexisting_paths
        .iter()
        .map(|path| relative_key(eval_root, path))
        .collect::<Result<Vec<_>, _>>()?;
    write_json(
        &baseline_dir.join(BASELINE_MANIFEST),
        &BaselineManifest { preexisting_files },
    )?;

    for source in preexisting_paths {
        let relative = source.strip_prefix(eval_root).map_err(|_| {
            PipelineError::Message(format!(
                "diff-scope baseline path {} is outside {}",
                source.display(),
                eval_root.display()
            ))
        })?;
        copy_entry(&source, &file_snapshot.join(relative))?;
    }
    Ok(())
}

fn measure_task_diff(eval_root: &Path, run_dir: &Path) -> Result<DiffScopeMetrics, PipelineError> {
    let baseline_dir = run_dir.join(BASELINE_DIR);
    let manifest: BaselineManifest =
        serde_json::from_str(&fs::read_to_string(baseline_dir.join(BASELINE_MANIFEST))?)?;
    let file_snapshot = baseline_dir.join(BASELINE_FILES);
    let mut candidates = BTreeSet::new();

    for relative in manifest.preexisting_files {
        if fs::symlink_metadata(file_snapshot.join(&relative)).is_err() {
            return Err(PipelineError::Message(format!(
                "diff-scope baseline is incomplete: missing snapshot for {relative}"
            )));
        }
        candidates.insert(relative);
    }
    let framework_outputs = eval_root.join(".eval-magic-outputs");
    for path in walk_files(eval_root, Some(&framework_outputs))? {
        candidates.insert(relative_key(eval_root, &path)?);
    }

    let mut metrics = DiffScopeMetrics::default();
    for relative in candidates {
        let before = file_snapshot.join(&relative);
        let after = eval_root.join(&relative);
        let old = file_content(&before)?;
        let new = file_content(&after)?;
        if old == new {
            continue;
        }
        metrics.files_touched += 1;
        let diff = match (old, new) {
            (FileContent::Regular(old), FileContent::Regular(new)) => Some(diff_bytes(&old, &new)),
            (FileContent::Missing, FileContent::Regular(new)) => Some(diff_bytes(&[], &new)),
            (FileContent::Regular(old), FileContent::Missing) => Some(diff_bytes(&old, &[])),
            _ => None,
        };
        if let Some(diff) = diff {
            metrics.lines_added += diff.lines_added;
            metrics.lines_removed += diff.lines_removed;
            metrics.hunks += diff.hunks;
        }
    }
    Ok(metrics)
}

#[derive(Debug, PartialEq, Eq)]
enum FileContent {
    Missing,
    Regular(Vec<u8>),
    Symlink(PathBuf),
}

fn file_content(path: &Path) -> Result<FileContent, PipelineError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileContent::Missing);
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        return Ok(FileContent::Symlink(fs::read_link(path)?));
    }
    if metadata.is_file() {
        return Ok(FileContent::Regular(fs::read(path)?));
    }
    Ok(FileContent::Missing)
}

fn walk_files(root: &Path, excluded_root: Option<&Path>) -> Result<Vec<PathBuf>, PipelineError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_excluded(entry, excluded_root))
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_type().is_file() || entry.file_type().is_symlink() => {
                Some(Ok(entry.into_path()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(PipelineError::Message(format!(
                "could not walk diff-scope path under {}: {error}",
                root.display()
            )))),
        })
        .collect()
}

fn is_excluded(entry: &DirEntry, excluded_root: Option<&Path>) -> bool {
    excluded_root.is_some_and(|excluded| entry.path().starts_with(excluded))
}

fn relative_key(root: &Path, path: &Path) -> Result<String, PipelineError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        PipelineError::Message(format!(
            "diff-scope path {} is outside {}",
            path.display(),
            root.display()
        ))
    })?;
    Ok(relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn copy_entry(source: &Path, destination: &Path) -> Result<(), PipelineError> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let target = fs::read_link(source)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, destination)?;
        #[cfg(windows)]
        if source.metadata().is_ok_and(|metadata| metadata.is_dir()) {
            std::os::windows::fs::symlink_dir(target, destination)?;
        } else {
            std::os::windows::fs::symlink_file(target, destination)?;
        }
    } else if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn byte_line_diff_counts_changes_and_zero_context_hunks() {
        let diff = diff_bytes(b"old\nsame\nbefore\n", b"new\nsame\nafter\n");
        assert_eq!(diff.lines_added, 2);
        assert_eq!(diff.lines_removed, 2);
        assert_eq!(diff.hunks, 2);
    }

    #[test]
    fn byte_line_diff_handles_empty_trailing_newline_and_non_utf8_inputs() {
        assert_eq!(diff_bytes(b"", b""), FileDiff::default());

        let trailing = diff_bytes(b"value", b"value\n");
        assert_eq!(trailing.lines_added, 1);
        assert_eq!(trailing.lines_removed, 1);
        assert_eq!(trailing.hunks, 1);

        let binary = diff_bytes(&[0xff, b'\n'], &[0xfe, b'\n']);
        assert_eq!(binary.lines_added, 1);
        assert_eq!(binary.lines_removed, 1);
        assert_eq!(binary.hunks, 1);
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
    fn baseline_measurement_counts_all_task_changes_except_framework_outputs() {
        let temp = tempfile::TempDir::new().unwrap();
        let eval_root = temp.path().join("env");
        let run_dir = temp.path().join("run");
        let outputs_dir = eval_root.join(".eval-magic-outputs/eval-e1/with_skill");
        fs::create_dir_all(eval_root.join("src")).unwrap();
        fs::create_dir_all(&outputs_dir).unwrap();
        fs::write(eval_root.join("src/changed.txt"), "old\nsame\n").unwrap();
        fs::write(eval_root.join("src/deleted.txt"), "gone\n").unwrap();
        fs::write(eval_root.join("framework.txt"), "before\n").unwrap();

        capture_task_baseline(&eval_root, &run_dir).unwrap();

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

        let metrics = measure_task_diff(&eval_root, &run_dir).unwrap();
        assert_eq!(
            metrics,
            DiffScopeMetrics {
                files_touched: 4,
                lines_added: 4,
                lines_removed: 3,
                hunks: 4,
            }
        );
    }
}
