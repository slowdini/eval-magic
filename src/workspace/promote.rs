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

use crate::core::{ConditionsRecord, Harness, run_git};
use crate::pipeline::run_slots;
use crate::workspace::teardown::PROMOTED_MARKER;
use crate::workspace::{WorkspaceError, now_iso8601, write_json};

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
    /// Directory used to resolve the committing repo's git HEAD for provenance.
    pub git_cwd: &'a Path,
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

    let baseline_dir = opts.skill_subdir.join("evals").join("baseline");
    let grading_dir = baseline_dir.join("grading");
    fs::create_dir_all(&grading_dir)?;

    fs::copy(&benchmark_src, baseline_dir.join("benchmark.json"))?;

    let (gradings_copied, missing_gradings) = copy_gradings(&iteration_dir, &grading_dir)?;

    let head = git_head(opts.git_cwd);
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
    let run_label = opts
        .label
        .or_else(|| conditions.and_then(|c| c.label.as_deref()))
        .unwrap_or("(none)");

    let lines = [
        format!("# Baseline — {}", opts.skill_name),
        String::new(),
        "Committed reference output from a canonical eval run. Regenerate with".to_string(),
        format!(
            "`eval-magic promote-baseline --iteration {}` after aggregating. The ephemeral workspace (run records, timing,",
            opts.iteration
        ),
        "dispatch files, produced outputs) stays gitignored under `skills-workspace/`".to_string(),
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
        format!("| Conditions | {conditions_cell} |"),
        format!("| Run timestamp | {timestamp} |"),
        format!("| Label | {run_label} |"),
        format!("| Promoted from commit | {head} |"),
        String::new(),
        "Files:".to_string(),
        "- `benchmark.json` — aggregate pass-rate / duration / token deltas.".to_string(),
        "- `grading/<eval-id>__<condition>.json` (multi-run cells add an `__r<k>` suffix per run) — assertion results and judge rationales."
            .to_string(),
        "- `NOTES.md` — operator-authored observations for this baseline (never overwritten by promote)."
            .to_string(),
        String::new(),
    ];
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
        let workspace_root = tmp.path().join("work").join("skills-workspace");
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
            harness: Harness::ClaudeCode,
            label: None,
            agent_model: None,
            judge_model: None,
            git_cwd: &f.skill_subdir,
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
        promote_baseline(&o).unwrap();

        let provenance =
            fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
        assert!(provenance.contains("Agent model | claude-haiku-4-5-20251001"));
        assert!(provenance.contains("Judge model | claude-opus-4-7"));
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

        let provenance =
            fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
        assert!(provenance.contains("Agent model | claude-haiku-4-5-20251001"));
        assert!(provenance.contains("Judge model | claude-opus-4-8"));
        assert!(provenance.contains("Label | canonical-run"));
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

        let provenance =
            fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
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
}
