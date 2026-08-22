//! Baseline promotion.
//!
//! Copy the durable, reference-worthy
//! subset of a workspace iteration (`benchmark.json`, per-run `grading.json`, a
//! `BASELINE.md` provenance file) into the skill's version-controlled
//! `evals/baseline/`, and drop a `.promoted.json` marker so `teardown` can
//! reclaim the iteration. Ephemeral scaffolding (dispatch/timing/run records,
//! produced outputs, transcripts) is intentionally left behind.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use crate::core::fs::write_json;
use crate::core::{ConditionsRecord, Harness, run_git};
use crate::pipeline::run_slots;
use crate::workspace::teardown::PROMOTED_MARKER;
use crate::workspace::{WorkspaceError, now_iso8601};

/// Inputs for [`promote_baseline`]. Borrowed for the duration of the call.
pub struct PromoteOptions<'a> {
    pub workspace_root: &'a Path,
    pub skill_name: &'a str,
    pub skill_subdir: &'a Path,
    pub iteration: u32,
    pub harness: Harness,
    pub label: Option<&'a str>,
    /// Operator-declared models for provenance. The runner never dispatches the
    /// agent/judge itself, so it cannot observe these — record what was used.
    pub agent_model: Option<&'a str>,
    pub judge_model: Option<&'a str>,
    pub responder_model: Option<&'a str>,
}

/// What [`promote_baseline`] wrote.
#[derive(Debug)]
pub struct PromoteResult {
    pub baseline_dir: PathBuf,
    pub gradings_copied: usize,
    /// Run slots whose `grading.json` was absent and therefore not copied — a
    /// sign the iteration was promoted before grading finished. Surfaced as a
    /// warning so the gap isn't silent.
    pub missing_gradings: usize,
    pub notes: NotesStatus,
}

/// How `NOTES.md` in the baseline dir was handled during promotion.
///
/// Promotion never overwrites operator-authored notes, but a baseline whose
/// notes describe a *previous* iteration is easy to ship by accident — the
/// caller should surface `RetainedFromPrior` as a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesStatus {
    /// A `NOTES.md` already existed and was left untouched.
    RetainedFromPrior,
    /// No `NOTES.md` existed; a stub was written for the operator to fill in.
    StubWritten,
}

/// Copy the durable subset of `iteration-<n>` into `<skill>/evals/baseline/` and
/// mark the iteration promoted. Errors if the iteration or its `benchmark.json`
/// is missing.
pub fn promote_baseline(opts: &PromoteOptions) -> Result<PromoteResult, WorkspaceError> {
    let iteration_dir = opts
        .workspace_root
        .join(opts.skill_name)
        .join(format!("iteration-{}", opts.iteration));
    if !iteration_dir.exists() {
        return Err(WorkspaceError::Message(format!(
            "not found: {} (build/grade iteration-{} first)",
            iteration_dir.display(),
            opts.iteration
        )));
    }

    let benchmark_src = iteration_dir.join("benchmark.json");
    if !benchmark_src.exists() {
        return Err(WorkspaceError::Message(format!(
            "missing benchmark.json in iteration-{} — run 'eval-magic aggregate' before promoting",
            opts.iteration
        )));
    }

    let conditions_src = iteration_dir.join("conditions.json");
    let conditions: Option<ConditionsRecord> = if conditions_src.exists() {
        Some(serde_json::from_str(&fs::read_to_string(&conditions_src)?)?)
    } else {
        None
    };

    // The baseline belongs to the skill this iteration measured, which the run
    // recorded. Deriving it from the operator's current selection instead would
    // write one skill's baseline into another whenever the two disagree — and the
    // operator can promote from anywhere, long after the run.
    let skill_subdir = recorded_skill_subdir(conditions.as_ref())?
        .unwrap_or_else(|| opts.skill_subdir.to_path_buf());
    let baseline_dir = skill_subdir.join("evals").join("baseline");
    let grading_dir = baseline_dir.join("grading");
    fs::create_dir_all(&grading_dir)?;

    fs::copy(&benchmark_src, baseline_dir.join("benchmark.json"))?;

    let (gradings_copied, missing_gradings) = copy_gradings(&iteration_dir, &grading_dir)?;

    let head = git_head(&skill_subdir);
    fs::write(
        baseline_dir.join("BASELINE.md"),
        provenance(opts, conditions.as_ref(), &head),
    )?;

    let notes = write_or_retain_notes(&baseline_dir, opts)?;

    // Mark the iteration as committed so `teardown` can safely reclaim its
    // workspace — without this marker teardown preserves it as uncommitted.
    write_json(
        &iteration_dir.join(PROMOTED_MARKER),
        &json!({
            "promoted_at": now_iso8601(),
            "baseline_dir": baseline_dir.to_string_lossy(),
            "commit": head,
        }),
    )?;

    Ok(PromoteResult {
        baseline_dir,
        gradings_copied,
        missing_gradings,
        notes,
    })
}

/// Leave an existing `NOTES.md` untouched (operator-authored), or write a stub
/// naming the promoted iteration so the convention is visible from the start.
fn write_or_retain_notes(
    baseline_dir: &Path,
    opts: &PromoteOptions,
) -> Result<NotesStatus, WorkspaceError> {
    let notes_path = baseline_dir.join("NOTES.md");
    if notes_path.exists() {
        return Ok(NotesStatus::RetainedFromPrior);
    }
    fs::write(
        &notes_path,
        format!(
            "# Notes — {}\n\nPromoted from iteration-{} at {}.\n\nRecord operator observations \
             for this baseline here (judge quirks, flaky evals, context for the deltas).\n",
            opts.skill_name,
            opts.iteration,
            now_iso8601(),
        ),
    )?;
    Ok(NotesStatus::StubWritten)
}

/// Copy each run's `grading.json` from every `eval-<id>/<condition>` cell into
/// `<grading_dir>/`, returning `(copied, missing)`. A flat `runs: 1` cell lands
/// at `<id>__<condition>.json`; a multi-run cell emits one
/// `<id>__<condition>__r<k>.json` per `run-<k>/`. `missing` counts run slots
/// whose `grading.json` is absent (an incomplete iteration). Entries are sorted
/// so the copy is deterministic.
fn copy_gradings(
    iteration_dir: &Path,
    grading_dir: &Path,
) -> Result<(usize, usize), WorkspaceError> {
    let mut copied = 0;
    let mut missing = 0;
    for eval_name in sorted_entry_names(iteration_dir) {
        let Some(eval_id) = eval_name.strip_prefix("eval-") else {
            continue;
        };
        let eval_dir = iteration_dir.join(&eval_name);
        if !eval_dir.is_dir() {
            continue;
        }
        for cond_name in sorted_entry_names(&eval_dir) {
            let cond_dir = eval_dir.join(&cond_name);
            if !cond_dir.is_dir() {
                continue;
            }
            // Walk every run slot so multi-run cells (`run-<k>/grading.json`)
            // are captured alongside flat `runs: 1` cells, just as `aggregate`
            // reads them.
            for slot in run_slots(&cond_dir) {
                let grading_src = slot.dir.join("grading.json");
                if !grading_src.exists() {
                    missing += 1;
                    continue;
                }
                let dest = match slot.run_index {
                    Some(k) => format!("{eval_id}__{cond_name}__r{k}.json"),
                    None => format!("{eval_id}__{cond_name}.json"),
                };
                fs::copy(&grading_src, grading_dir.join(dest))?;
                copied += 1;
            }
        }
    }
    Ok((copied, missing))
}

/// Directory entry names, sorted. Missing/unreadable dirs yield `[]`.
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

/// `git rev-parse --short HEAD` in `cwd`, or `"unknown"` when git is
/// unavailable / `cwd` isn't a repo — provenance stays useful without it.
fn git_head(cwd: &Path) -> String {
    let res = run_git(&["rev-parse", "--short", "HEAD"], cwd);
    if res.status == Some(0) {
        String::from_utf8_lossy(&res.stdout).trim().to_string()
    } else {
        "unknown".to_string()
    }
}

/// Serialize an enum that renders to a string (`Harness`, `Mode`) to its
/// kebab-case label via serde, so we never hardcode variant spellings.
fn label(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_string())
}

/// The skill directory the run recorded, when it recorded one.
///
/// A pointer to a directory that has since moved is a hard failure rather than a
/// fall back to the caller's selection: quietly writing one skill's baseline into
/// another is the outcome worth refusing.
fn recorded_skill_subdir(
    conditions: Option<&ConditionsRecord>,
) -> Result<Option<PathBuf>, WorkspaceError> {
    let Some(recorded) = conditions
        .and_then(|c| c.skill_source.as_ref())
        .and_then(|skill| skill.source.resolved_path.as_deref())
    else {
        return Ok(None);
    };
    let path = PathBuf::from(recorded);
    if !path.is_dir() {
        return Err(WorkspaceError::Message(format!(
            "the skill this iteration measured is no longer at {recorded}. Restore it, or promote \
             from a workspace whose run recorded the skill you mean."
        )));
    }
    Ok(Some(path))
}

/// The provenance-table row naming the skill under test, or an empty string for
/// an iteration recorded before skills were sourced.
///
/// The gap this closes: a report could pin the codebase commit while the skill
/// side was "whatever was on disk at the time". Where uncommitted work was in
/// what ran, the revision alone does not identify it, and the row says so.
fn skill_source_row(conditions: Option<&ConditionsRecord>) -> String {
    let Some(skill) = conditions.and_then(|c| c.skill_source.as_ref()) else {
        return String::new();
    };
    let source = &skill.source;
    let mut cell = source
        .resolved_path
        .clone()
        .unwrap_or_else(|| source.source.clone());
    if let Some(revision) = &source.revision {
        let short: String = revision.chars().take(7).collect();
        cell.push_str(&format!(" ({short})"));
    }
    if source.dirty {
        cell.push_str(
            " — uncommitted changes were in what ran, so the revision alone does not identify it",
        );
    }
    if let Some(origin) = &source.origin_url {
        cell.push_str(&format!("; origin {origin}"));
    }
    if !skill.siblings.is_empty() {
        cell.push_str(&format!("; staged alongside {}", skill.siblings.join(", ")));
    }
    format!("| Skill source | {cell} |")
}

/// Provenance-table rows naming each codebase the iteration ran against, or an
/// empty string when it ran against none.
///
/// A reader deciding whether to believe a published baseline needs the commit,
/// not the ref: a branch has moved by the time they read it. Where the source is
/// a directory on the machine that ran it, the row says so — that reader cannot
/// resolve the path, and the row should not imply otherwise.
fn codebase_rows(conditions: Option<&ConditionsRecord>) -> String {
    let codebases = conditions.map(|c| c.codebases.as_slice()).unwrap_or(&[]);
    if codebases.is_empty() {
        return String::new();
    }
    let multiple = codebases.len() > 1;
    codebases
        .iter()
        .map(|used| {
            // One codebase needs no disambiguation; several do, and the eval ids
            // are what tie a row to the cells it covers.
            let label = if multiple {
                format!("Codebase ({})", used.evals.join(", "))
            } else {
                "Codebase".to_string()
            };
            let mut cell = used.codebase.source.clone();
            if let Some(reference) = &used.codebase.reference {
                cell.push('@');
                cell.push_str(reference);
            }
            if let Some(revision) = &used.codebase.revision {
                let short: String = revision.chars().take(7).collect();
                cell.push_str(&format!(" ({short})"));
            }
            if used.codebase.host_local {
                cell.push_str(" — host-local path, not reproducible from this config alone");
                if let Some(origin) = &used.codebase.origin_url {
                    cell.push_str(&format!("; origin {origin}"));
                }
            }
            format!("| {label} | {cell} |")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build the `BASELINE.md` provenance document — byte-for-byte the layout of
/// `promote-baseline.ts`.
fn provenance(opts: &PromoteOptions, conditions: Option<&ConditionsRecord>, head: &str) -> String {
    let mode = conditions
        .map(|c| label(&c.mode))
        .unwrap_or_else(|| "unknown".to_string());
    let timestamp = conditions
        .map(|c| c.timestamp.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let condition_names: Vec<&str> = conditions
        .map(|c| c.conditions.iter().map(|e| e.name.as_str()).collect())
        .unwrap_or_default();
    let conditions_cell = if condition_names.is_empty() {
        "unknown".to_string()
    } else {
        condition_names.join(", ")
    };
    let harness = label(&opts.harness);

    // Provenance precedence: explicit promote-baseline flag → value recorded in
    // the iteration's conditions.json (set via `run`) → placeholder.
    let agent_model = opts
        .agent_model
        .or_else(|| conditions.and_then(|c| c.agent_model.as_deref()))
        .unwrap_or("unspecified");
    let judge_model = opts
        .judge_model
        .or_else(|| conditions.and_then(|c| c.judge_model.as_deref()))
        .unwrap_or("unspecified");
    let responder_model = opts
        .responder_model
        .or_else(|| conditions.and_then(|c| c.responder_model.as_deref()))
        .unwrap_or("unspecified");
    let run_label = opts
        .label
        .or_else(|| conditions.and_then(|c| c.label.as_deref()))
        .unwrap_or("(none)");

    let codebase_rows = codebase_rows(conditions);
    let skill_source_row = skill_source_row(conditions);

    let lines = [
        format!("# Baseline — {}", opts.skill_name),
        String::new(),
        "Committed reference output from a canonical eval run. Regenerate with".to_string(),
        format!(
            "`eval-magic promote-baseline --iteration {}` after aggregating. The ephemeral workspace (run records, timing,",
            opts.iteration
        ),
        "dispatch files, produced outputs) stays gitignored under `.eval-magic/`".to_string(),
        "and is reclaimable by `eval-magic teardown` once promoted (this commit's marker)."
            .to_string(),
        String::new(),
        "| Field | Value |".to_string(),
        "|-------|-------|".to_string(),
        format!("| Mode | {mode} |"),
        format!("| Iteration | iteration-{} |", opts.iteration),
        format!("| Harness | {harness} |"),
        format!("| Agent model | {agent_model} |"),
        format!("| Judge model | {judge_model} |"),
        format!("| Responder model | {responder_model} |"),
        format!("| Conditions | {conditions_cell} |"),
        format!("| Run timestamp | {timestamp} |"),
        format!("| Label | {run_label} |"),
        skill_source_row,
        codebase_rows,
        format!("| Promoted from commit | {head} |"),
        String::new(),
        "Files:".to_string(),
        "- `benchmark.json` — aggregate pass-rate / duration / token deltas plus per-assertion pass counts."
            .to_string(),
        "- `grading/<eval-id>__<condition>.json` (multi-run cells add an `__r<k>` suffix per run) — assertion results and judge rationales."
            .to_string(),
        "- `NOTES.md` — operator-authored observations for this baseline (never overwritten by promote)."
            .to_string(),
        String::new(),
    ];
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests;
