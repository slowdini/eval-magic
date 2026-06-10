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
            "missing benchmark.json in iteration-{} — run 'skill-eval aggregate' before promoting",
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

    let gradings_copied = copy_gradings(&iteration_dir, &grading_dir)?;

    let head = git_head(opts.git_cwd);
    fs::write(
        baseline_dir.join("BASELINE.md"),
        provenance(opts, conditions.as_ref(), &head),
    )?;

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
    })
}

/// Copy every `eval-<id>/<condition>/grading.json` into
/// `<grading_dir>/<id>__<condition>.json`, returning the count. Entries are
/// sorted so the copy is deterministic.
fn copy_gradings(iteration_dir: &Path, grading_dir: &Path) -> Result<usize, WorkspaceError> {
    let mut copied = 0;
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
            let grading_src = cond_dir.join("grading.json");
            if !grading_src.exists() {
                continue;
            }
            fs::copy(
                &grading_src,
                grading_dir.join(format!("{eval_id}__{cond_name}.json")),
            )?;
            copied += 1;
        }
    }
    Ok(copied)
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

    let lines = [
        format!("# Baseline — {}", opts.skill_name),
        String::new(),
        "Committed reference output from a canonical eval run. Regenerate with".to_string(),
        format!(
            "`skill-eval promote-baseline --skill {} --iteration <N>` after aggregating. The ephemeral workspace (run records, timing,",
            opts.skill_name
        ),
        "dispatch files, produced outputs) stays gitignored under `skills-workspace/`".to_string(),
        "and is reclaimable by `skill-eval teardown` once promoted (this commit's marker)."
            .to_string(),
        String::new(),
        "| Field | Value |".to_string(),
        "|-------|-------|".to_string(),
        format!("| Mode | {mode} |"),
        format!("| Iteration | iteration-{} |", opts.iteration),
        format!("| Harness | {harness} |"),
        format!(
            "| Agent model | {} |",
            opts.agent_model.unwrap_or("unspecified")
        ),
        format!(
            "| Judge model | {} |",
            opts.judge_model.unwrap_or("unspecified")
        ),
        format!("| Conditions | {conditions_cell} |"),
        format!("| Run timestamp | {timestamp} |"),
        format!("| Label | {} |", opts.label.unwrap_or("(none)")),
        format!("| Promoted from commit | {head} |"),
        String::new(),
        "Files:".to_string(),
        "- `benchmark.json` — aggregate pass-rate / duration / token deltas.".to_string(),
        "- `grading/<eval-id>__<condition>.json` — per-run assertion results and judge rationales."
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

    #[test]
    fn fails_clearly_when_iteration_dir_is_missing() {
        let f = fixture(1); // creates iteration-1, but we promote iteration-9
        let err = promote_baseline(&opts(&f, 9)).unwrap_err();
        assert!(matches!(err, WorkspaceError::Message(_)));
        assert!(err.to_string().contains("iteration-9"));
    }
}
