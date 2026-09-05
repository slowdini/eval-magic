//! Deterministic final-environment diff metrics, measured with Git.
//!
//! Every task environment is a Git repository marked with [`BASELINE_REF`] at
//! the state the agent started from, so the difference between that ref and the
//! finished working tree *is* the measurement. Nothing is copied and nothing is
//! walked: Git already knows what changed, and it knows it while honoring the
//! codebase's own `.gitignore` and the runner's exclusion of what the framework
//! itself owns (its outputs dir and the task-local scratch directory dispatch
//! prompts designate — see [`crate::sandbox::framework_owned_entries`]).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::fs::write_json;
use crate::core::{BASELINE_REF, IsolatedGit};
use crate::pipeline::error::PipelineError;

const RESULT_FILE: &str = "diff-scope.json";
/// The diff itself, beside the metrics that summarize it. Named in the record
/// rather than only known by convention, so a reader of `diff-scope.json` can
/// find it without knowing this constant.
const PATCH_FILE: &str = "diff.patch";
/// How much of a run's diff is captured. A safety valve against an agent that
/// rewrites a whole tree, not a judging budget — a realistic task diff is far
/// below it, and bounding evidence for a judge is a separate concern.
const PATCH_BYTE_LIMIT: usize = 1_048_576;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffScopeMetrics {
    pub files_touched: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub hunks: u64,
}

/// One run's complete diff evidence: the counters, and where the patch is.
///
/// Written as `diff-scope.json`. The counters stay flattened at the top level —
/// they are what `benchmark.json` aggregates and what a `diff_scope` assertion
/// grades, and a reader that wants only those can still deserialize
/// [`DiffScopeMetrics`] straight from this artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffScopeRecord {
    #[serde(flatten)]
    pub metrics: DiffScopeMetrics,
    /// Every changed file, ordered by path as Git reports them.
    pub files: Vec<ChangedFile>,
    pub patch: PatchRecord,
}

/// One file the task changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    /// Environment-relative path, spelled with forward slashes as Git spells it.
    pub path: String,
    pub status: ChangeStatus,
    pub lines_added: u64,
    pub lines_removed: u64,
}

/// What happened to a changed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
}

/// Where a run's patch is and whether it is the whole diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchRecord {
    /// Name of the patch file beside this record.
    pub path: String,
    pub bytes: u64,
    /// True when the diff exceeded the cap and the file carries a marker in
    /// place of the rest. A grader reading a truncated patch is reading part of
    /// the story, and has to be able to tell.
    pub truncated: bool,
}

impl DiffScopeMetrics {
    pub fn lines_changed(self) -> u64 {
        self.lines_added.saturating_add(self.lines_removed)
    }
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiffScopeSummary {
    pub measured: usize,
    pub reused: usize,
    pub missing_baseline: usize,
    pub shared_environment: usize,
    /// Per-task detail behind the counters above (which task lacked an
    /// `eval_root`, a baseline, or had a shared environment). Collected here
    /// rather than printed so the stage stays silent and the CLI owns every
    /// user-facing line.
    pub warnings: Vec<String>,
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
            summary.warnings.push(format!(
                "{}/{}{run_label} has no eval_root — diff-scope unavailable; rebuild the iteration to capture metrics",
                task.eval_id, task.condition
            ));
            summary.missing_baseline += 1;
            continue;
        };
        if root_counts.get(eval_root).copied().unwrap_or_default() != 1 {
            summary.warnings.push(format!(
                "{}/{}{run_label} shares eval_root with another task — diff-scope unavailable; rebuild the iteration for task-scoped environments",
                task.eval_id, task.condition
            ));
            summary.shared_environment += 1;
            continue;
        }
        if !has_baseline(Path::new(eval_root))? {
            summary.warnings.push(format!(
                "{}/{}{run_label} has no {BASELINE_REF} in its environment — diff-scope unavailable; the environment was removed, or the iteration predates the baseline ref and needs rebuilding",
                task.eval_id, task.condition
            ));
            summary.missing_baseline += 1;
            continue;
        }
        if run_dir.join("command-checks").exists() {
            return Err(PipelineError::Message(format!(
                "cannot capture diff scope for {}/{}{run_label} after command checks have run; rebuild the iteration",
                task.eval_id, task.condition
            )));
        }

        let record = measure_task_diff(Path::new(eval_root), run_dir)?;
        crate::validation::validate_against_schema::<DiffScopeRecord>(
            crate::validation::SchemaName::DiffScope,
            &serde_json::to_value(&record)?,
            &result_path.to_string_lossy(),
        )?;
        write_json(&result_path, &record)?;
        summary.measured += 1;
    }
    Ok(summary)
}

/// Whether `eval_root` still carries the ref a measurement is taken against.
///
/// False for a torn-down environment, a root that is not a repository, and an
/// iteration built before the baseline ref existed — all reported gaps rather
/// than failures, so one unmeasurable task does not stop the stage. A host that
/// cannot give git an isolated configuration is a different thing entirely, and
/// errors rather than being reported as one more missing baseline.
fn has_baseline(eval_root: &Path) -> Result<bool, PipelineError> {
    let git = IsolatedGit::new().map_err(PipelineError::Message)?;
    Ok(git
        .run(
            eval_root,
            &["rev-parse", "--verify", "--quiet", BASELINE_REF],
            &[],
        )
        .status
        == Some(0))
}

/// Measure the final environment against the state the agent started from.
///
/// The environment is a Git repository whose start state is [`BASELINE_REF`], so
/// Git supplies the metrics: a scratch index seeded from that ref, brought up to
/// the working tree, is exactly "everything the agent changed". Creations,
/// modifications, and deletions all fall out of one `git add`, and untracked
/// creations are not missed.
///
/// The scratch index lives outside the repository so the environment's own index
/// and `HEAD` are untouched — an eval may legitimately have run `git` itself. The
/// blobs `git add` writes do land in the environment's object store; measurement
/// runs post-dispatch against a disposable artifact, and re-running it over the
/// same working tree yields the same tree, so that is harmless.
fn measure_task_diff(eval_root: &Path, run_dir: &Path) -> Result<DiffScopeRecord, PipelineError> {
    let git = IsolatedGit::new().map_err(PipelineError::Message)?;
    let scratch = tempfile::TempDir::new()?;
    let index = scratch.path().join("index").to_string_lossy().into_owned();
    let env = [("GIT_INDEX_FILE", index.as_str())];

    git_checked(&git, eval_root, &["read-tree", BASELINE_REF], &env)?;
    // Unforced, so the codebase's own `.gitignore` and the `.git/info/exclude`
    // entries for the framework-owned paths all hold — the same rules the
    // baseline commit was built under. A path the runner force-added despite
    // those rules is already tracked by `read-tree`, and stays measured.
    git_checked(&git, eval_root, &["add", "--all", "--", "."], &env)?;
    let measured = git_checked(&git, eval_root, &["write-tree"], &env)?;
    let measured = String::from_utf8_lossy(&measured).trim().to_string();

    let numstat = git_checked(
        &git,
        eval_root,
        &diff_args(&["--numstat", "-z"], &measured),
        &env,
    )?;
    let statuses = git_checked(
        &git,
        eval_root,
        &diff_args(&["--name-status", "-z"], &measured),
        &env,
    )?;
    let zero_context = git_checked(
        &git,
        eval_root,
        &diff_args(&["--unified=0"], &measured),
        &env,
    )?;
    let files = changed_files(&numstat, &statuses)?;
    let metrics = DiffScopeMetrics {
        files_touched: files.len() as u64,
        lines_added: files.iter().map(|file| file.lines_added).sum(),
        lines_removed: files.iter().map(|file| file.lines_removed).sum(),
        hunks: count_hunks(&zero_context),
    };

    let patch = git_checked(
        &git,
        eval_root,
        &diff_args(&["--unified=3"], &measured),
        &env,
    )?;
    let (captured, truncated) = truncate_patch(&patch, PATCH_BYTE_LIMIT);
    fs::write(run_dir.join(PATCH_FILE), &captured)?;
    Ok(DiffScopeRecord {
        metrics,
        files,
        patch: PatchRecord {
            path: PATCH_FILE.to_string(),
            bytes: captured.len() as u64,
            truncated,
        },
    })
}

/// `patch` capped at `limit`, cut on a line boundary so the last diff line kept
/// is whole, with a marker in place of the rest.
///
/// The marker is unconditional once the cap is crossed: a patch that stops
/// early and does not say so reads as a complete, smaller diff, and a grader
/// would draw the wrong conclusion from it. A single line longer than the whole
/// cap has no boundary to cut on, so it is cut at the cap — an uncapped
/// artifact is the thing being prevented.
fn truncate_patch(patch: &[u8], limit: usize) -> (Vec<u8>, bool) {
    if patch.len() <= limit {
        return (patch.to_vec(), false);
    }
    let head = &patch[..limit];
    let end = match head.iter().rposition(|byte| *byte == b'\n') {
        Some(newline) => newline + 1,
        None => limit,
    };
    let mut captured = patch[..end].to_vec();
    captured.extend_from_slice(
        format!(
            "[eval-magic] patch truncated at {limit} bytes of {}; the remainder is not captured\n",
            patch.len()
        )
        .as_bytes(),
    );
    (captured, true)
}

/// A diff of the baseline against `measured`, with every configurable influence
/// on the numbers pinned.
///
/// `--no-renames` because `diff.renames` defaults on: a detected rename reports
/// one entry with no line changes, where a rename is two touched files — one
/// created and one deleted. `--no-ext-diff` and `--no-textconv` keep a sourced
/// codebase's `.gitattributes` from deciding what a measurement sees.
fn diff_args<'a>(format: &[&'a str], measured: &'a str) -> Vec<&'a str> {
    let mut args = vec!["diff", "--no-renames", "--no-ext-diff", "--no-textconv"];
    args.extend_from_slice(format);
    args.push(BASELINE_REF);
    args.push(measured);
    args
}

/// Every changed file, from the two views Git offers of one diff.
///
/// `--numstat -z` carries the line counts as `added\tremoved\tpath` per record;
/// `--name-status -z` carries the status as a `status`, `path` pair. Neither
/// format offers both, and no single `git diff` invocation emits both, so they
/// are joined by path here.
fn changed_files(numstat: &[u8], name_status: &[u8]) -> Result<Vec<ChangedFile>, PipelineError> {
    // Keyed by the same lossy conversion the numstat side uses. A path Git spells
    // in bytes that are not UTF-8 has to reach both sides identically, or it
    // joins against nothing and loses its status.
    let mut statuses: HashMap<String, ChangeStatus> = HashMap::new();
    let mut fields = name_status
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        statuses.insert(
            String::from_utf8_lossy(path).into_owned(),
            change_status(status),
        );
    }

    let mut files = Vec::new();
    for record in numstat.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(record);
        let mut columns = text.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (columns.next(), columns.next(), columns.next())
        else {
            return Err(PipelineError::Message(format!(
                "could not read a diff-scope numstat record: {text:?}"
            )));
        };
        files.push(ChangedFile {
            path: path.to_string(),
            status: statuses
                .get(path)
                .copied()
                .unwrap_or(ChangeStatus::Modified),
            lines_added: parse_count(added, &text)?,
            lines_removed: parse_count(removed, &text)?,
        });
    }
    Ok(files)
}

/// Git's status letter for a path. Renames and copies are off, and a tree diff
/// has no unmerged entries, so what remains beyond added and deleted is a change
/// to a path that existed before — a content edit, a mode change, or a swap
/// between a file and a symlink.
fn change_status(letter: &[u8]) -> ChangeStatus {
    match letter.first() {
        Some(b'A') => ChangeStatus::Added,
        Some(b'D') => ChangeStatus::Deleted,
        _ => ChangeStatus::Modified,
    }
}

fn parse_count(field: &str, record: &str) -> Result<u64, PipelineError> {
    if field == "-" {
        return Ok(0);
    }
    field.parse().map_err(|_| {
        PipelineError::Message(format!(
            "could not read a diff-scope line count from {record:?}"
        ))
    })
}

/// Contiguous non-equal groups, with zero context: at `--unified=0` every `@@`
/// header is one such group. No content line can be mistaken for one — a diff
/// prefixes those with `+`, `-`, or a space.
fn count_hunks(patch: &[u8]) -> u64 {
    patch
        .split(|byte| *byte == b'\n')
        .filter(|line| line.starts_with(b"@@"))
        .count() as u64
}

fn git_checked(
    git: &IsolatedGit,
    cwd: &Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<Vec<u8>, PipelineError> {
    let output = git.run(cwd, args, env);
    if output.status == Some(0) {
        return Ok(output.stdout);
    }
    Err(PipelineError::Message(format!(
        "git {} failed in {}: {}",
        args.join(" "),
        cwd.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

#[cfg(test)]
mod tests;
