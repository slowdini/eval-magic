//! Stage 5 — `aggregate`.
//!
//! Compares exactly two conditions: collects
//! `pass_rate` (from `grading.json`), `total_tokens`/`duration_ms` (from
//! `timing.json`), raw per-run diff scope, and the skill-invocation determination
//! per condition; computes mean/stddev and the `a - b` delta; accumulates validity
//! warnings (mixed timing sources, sub-100% invocation rate, stray-write
//! violations + live-source reads, guard denials, permission-denied tool calls,
//! plugin shadows); and writes `benchmark.json`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::adapters::skill_shadow::PluginShadowArtifact;
use crate::adapters::{adapter_for, shadow_validity_warnings};
use crate::core::{ConditionsRecord, GradingResult, Mode, TimingRecord, TimingSource};
use crate::pipeline::DiffScopeMetrics;
use crate::pipeline::error::PipelineError;
use crate::pipeline::git_isolation;
use crate::pipeline::guard_denials::GuardDenialsReport;
use crate::pipeline::io::{now_iso8601, write_json};
use crate::pipeline::permission_denials;
use crate::pipeline::slots::run_slots;
use crate::validation::{SchemaName, validate_against_schema};

/// Mean of a series (0 for an empty series).
fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

/// Population standard deviation about `m` (0 for fewer than two samples).
fn stddev(values: &[f64], m: f64) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let variance = values.iter().map(|x| (x - m).powi(2)).sum::<f64>() / values.len() as f64;
    variance.sqrt()
}

/// Round `n` to `dp` decimal places.
fn round(n: f64, dp: i32) -> f64 {
    let p = 10f64.powi(dp);
    (n * p).round() / p
}

/// Mean/stddev/n for a series, each rounded to `dp` places.
fn stats(values: &[f64], dp: i32) -> Stats {
    let m = mean(values);
    Stats {
        mean: round(m, dp),
        stddev: round(stddev(values, m), dp),
        n: values.len(),
    }
}

/// Summary statistics for one metric.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Stats {
    pub mean: f64,
    pub stddev: f64,
    /// Available sample count. `0` means unavailable, not a measured zero.
    pub n: usize,
}

/// Per-condition rollup. Skill-invocation fields appear only when the condition
/// had the skill loaded.
#[derive(Debug, Clone, Serialize)]
struct ConditionSummary {
    pass_rate: Stats,
    duration_ms: Stats,
    total_tokens: Stats,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill_invocation_n: Option<usize>,
    /// Present (possibly `null`) only when the skill was loaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    skill_invocation_rate: Option<Option<f64>>,
}

/// The `a - b` differences between the two compared conditions.
#[derive(Debug, Clone, Serialize)]
struct Delta {
    direction: String,
    pass_rate: f64,
    duration_ms: f64,
    total_tokens: f64,
}

/// The full `benchmark.json`.
#[derive(Debug, Clone, Serialize)]
pub struct Benchmark {
    pub generated: String,
    pub mode: Mode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<String>,
    pub conditions_compared: Vec<String>,
    pub missing_gradings: usize,
    pub validity_warnings: Vec<String>,
    pub run_summary: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_scope: Option<Value>,
    delta: Delta,
}

#[derive(Debug, Serialize)]
struct DiffScopeRun {
    eval_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_index: Option<u32>,
    #[serde(flatten)]
    metrics: DiffScopeMetrics,
}

/// Per-condition accumulators.
#[derive(Default)]
struct Bucket {
    pass_rates: Vec<f64>,
    durations: Vec<f64>,
    tokens: Vec<f64>,
    skill_invoked: Vec<bool>,
    had_skill_loaded: bool,
}

/// `stray-writes.json` runs, read leniently (only finding counts matter).
#[derive(Debug, Deserialize)]
struct StrayReport {
    #[serde(default)]
    runs: Vec<StrayRun>,
}

#[derive(Debug, Deserialize)]
struct StrayRun {
    eval_id: String,
    condition: String,
    #[serde(default)]
    violations: Vec<Value>,
    #[serde(default)]
    live_source_reads: Vec<Value>,
}

/// Compute and write `benchmark.json` for the iteration. Requires exactly two
/// conditions and at least one `eval-*` directory.
pub fn aggregate(
    iteration_dir: &Path,
    conditions: &ConditionsRecord,
) -> Result<Benchmark, PipelineError> {
    let condition_names: Vec<String> = conditions
        .conditions
        .iter()
        .map(|c| c.name.clone())
        .collect();
    if condition_names.len() != 2 {
        return Err(PipelineError::Message(format!(
            "expected exactly 2 conditions, got {}",
            condition_names.len()
        )));
    }

    let mut eval_dirs: Vec<String> = fs::read_dir(iteration_dir)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("eval-").then_some(name)
        })
        .collect();
    eval_dirs.sort();
    if eval_dirs.is_empty() {
        return Err(PipelineError::Message(
            "no eval directories found".to_string(),
        ));
    }

    let mut by_condition: HashMap<String, Bucket> = HashMap::new();
    for c in &conditions.conditions {
        by_condition.insert(
            c.name.clone(),
            Bucket {
                had_skill_loaded: c.skill_path.is_some(),
                ..Bucket::default()
            },
        );
    }

    let mut missing_gradings = 0usize;
    let mut timing_sources: HashSet<String> = HashSet::new();
    let mut diff_scope_by_condition: HashMap<String, Vec<DiffScopeRun>> = condition_names
        .iter()
        .map(|condition| (condition.clone(), Vec::new()))
        .collect();
    let mut missing_diff_scopes = Vec::new();

    for eval_dir in &eval_dirs {
        for cond in &condition_names {
            let cond_dir = iteration_dir.join(eval_dir).join(cond);
            for slot in run_slots(&cond_dir) {
                let grading_path = slot.dir.join("grading.json");
                let timing_path = slot.dir.join("timing.json");
                let diff_scope_path = slot.dir.join("diff-scope.json");
                if diff_scope_path.exists() {
                    let metrics = validate_against_schema(
                        SchemaName::DiffScope,
                        &serde_json::from_str(&fs::read_to_string(&diff_scope_path)?)?,
                        &diff_scope_path.to_string_lossy(),
                    )?;
                    diff_scope_by_condition
                        .get_mut(cond)
                        .expect("condition diff-scope bucket")
                        .push(DiffScopeRun {
                            eval_id: eval_dir
                                .strip_prefix("eval-")
                                .unwrap_or(eval_dir)
                                .to_string(),
                            run_index: slot.run_index,
                            metrics,
                        });
                } else {
                    let run = slot
                        .run_index
                        .map(|index| format!("/run-{index}"))
                        .unwrap_or_default();
                    missing_diff_scopes.push(format!("{eval_dir}/{cond}{run}"));
                }

                if !grading_path.exists() {
                    let run = slot
                        .run_index
                        .map(|k| format!("/run-{k}"))
                        .unwrap_or_default();
                    eprintln!("warn: missing grading for {eval_dir}/{cond}{run}");
                    missing_gradings += 1;
                    continue;
                }
                let grading: GradingResult =
                    serde_json::from_str(&fs::read_to_string(&grading_path)?)?;
                let bucket = by_condition.get_mut(cond).expect("condition bucket");
                bucket.pass_rates.push(grading.summary.pass_rate);
                if let Some(meta) = &grading.meta_summary
                    && let Some(invoked) = meta.skill_invoked
                {
                    bucket.skill_invoked.push(invoked);
                }

                if timing_path.exists() {
                    let timing: TimingRecord =
                        serde_json::from_str(&fs::read_to_string(&timing_path)?)?;
                    let has_tokens = matches!(timing.total_tokens, Some(Some(_)));
                    let has_duration = matches!(timing.duration_ms, Some(Some(_)));
                    if let Some(Some(tokens)) = timing.total_tokens {
                        bucket.tokens.push(tokens as f64);
                    }
                    if let Some(Some(duration)) = timing.duration_ms {
                        bucket.durations.push(duration as f64);
                    }
                    if has_tokens || has_duration {
                        timing_sources.insert(timing_source_label(timing.source));
                    }
                }
            }
        }
    }

    // Build the per-condition summaries, preserving condition order.
    let mut run_summary = serde_json::Map::new();
    let mut summaries: HashMap<String, ConditionSummary> = HashMap::new();
    for cond in &condition_names {
        let bucket = &by_condition[cond];
        let (skill_invocation_n, skill_invocation_rate) = if bucket.had_skill_loaded {
            let n = bucket.skill_invoked.len();
            let rate = if n == 0 {
                None
            } else {
                let passed = bucket.skill_invoked.iter().filter(|&&b| b).count();
                Some(round(passed as f64 / n as f64, 3))
            };
            (Some(n), Some(rate))
        } else {
            (None, None)
        };
        let summary = ConditionSummary {
            pass_rate: stats(&bucket.pass_rates, 3),
            duration_ms: stats(&bucket.durations, 0),
            total_tokens: stats(&bucket.tokens, 0),
            skill_invocation_n,
            skill_invocation_rate,
        };
        run_summary.insert(cond.clone(), serde_json::to_value(&summary)?);
        summaries.insert(cond.clone(), summary);
    }

    let a = &condition_names[0];
    let b = &condition_names[1];
    let sa = &summaries[a];
    let sb = &summaries[b];
    let delta = Delta {
        direction: format!("{a} - {b}"),
        pass_rate: round(sa.pass_rate.mean - sb.pass_rate.mean, 3),
        duration_ms: round(sa.duration_ms.mean - sb.duration_ms.mean, 0),
        total_tokens: round(sa.total_tokens.mean - sb.total_tokens.mean, 0),
    };

    let mut validity_warnings: Vec<String> = Vec::new();
    if timing_sources.len() > 1 {
        let mut sorted: Vec<&String> = timing_sources.iter().collect();
        sorted.sort();
        let joined = sorted
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        validity_warnings.push(format!(
            "runs mix timing sources ({joined}) — completion events and transcript extractors \
             may use different harness-specific accounting, so the token/duration delta may \
             compare different metrics. Re-record one side or read the delta as a rough signal \
             only."
        ));
    }
    let (n_a, n_b) = (
        by_condition[a].pass_rates.len(),
        by_condition[b].pass_rates.len(),
    );
    if n_a != n_b {
        validity_warnings.push(format!(
            "conditions have uneven run counts ({a}: {n_a}, {b}: {n_b}) — the delta compares \
             differently-sized samples, weakening the comparison."
        ));
    }
    for cond in &condition_names {
        let summary = &summaries[cond];
        let graded_n = summary.pass_rate.n;
        if summary.total_tokens.n < graded_n || summary.duration_ms.n < graded_n {
            validity_warnings.push(format!(
                "condition '{cond}' has incomplete timing samples (total_tokens: {}/{graded_n}; \
                 duration_ms: {}/{graded_n}) — metric stats and deltas use only available \
                 samples; n: 0 is unavailable, not a measured zero.",
                summary.total_tokens.n, summary.duration_ms.n
            ));
        }
        if let Some(Some(rate)) = summaries[cond].skill_invocation_rate
            && rate < 1.0
        {
            let n = summaries[cond].skill_invocation_n.unwrap_or(0);
            validity_warnings.push(format!(
                "condition '{cond}' had skill loaded but invocation rate {:.0}% ({n} runs \
                 checked) — substantive results may not reflect skill effectiveness.",
                rate * 100.0
            ));
        }
    }

    git_isolation::collect_warnings(iteration_dir, &mut validity_warnings);
    collect_stray_warnings(iteration_dir, &mut validity_warnings);
    collect_guard_denial_warnings(iteration_dir, &mut validity_warnings);
    permission_denials::collect_warnings(iteration_dir, &mut validity_warnings);
    collect_shadow_warnings(iteration_dir, conditions, &mut validity_warnings);

    let has_diff_scopes = diff_scope_by_condition
        .values()
        .any(|runs| !runs.is_empty());
    if has_diff_scopes && !missing_diff_scopes.is_empty() {
        validity_warnings.push(format!(
            "{} run(s) are missing diff-scope metrics ({}) — compare scope only across the listed runs.",
            missing_diff_scopes.len(),
            missing_diff_scopes.join(", ")
        ));
    }
    let diff_scope = has_diff_scopes.then(|| {
        let mut by_condition = serde_json::Map::new();
        for condition in &condition_names {
            by_condition.insert(
                condition.clone(),
                serde_json::to_value(
                    diff_scope_by_condition
                        .remove(condition)
                        .expect("condition diff-scope bucket"),
                )
                .expect("diff-scope runs serialize"),
            );
        }
        Value::Object(by_condition)
    });

    let benchmark = Benchmark {
        generated: now_iso8601(),
        mode: conditions.mode,
        baseline: conditions.baseline.clone(),
        conditions_compared: vec![a.clone(), b.clone()],
        missing_gradings,
        validity_warnings,
        run_summary: Value::Object(run_summary),
        diff_scope,
        delta,
    };

    let out_path = iteration_dir.join("benchmark.json");
    validate_against_schema::<Value>(
        SchemaName::Benchmark,
        &serde_json::to_value(&benchmark)?,
        &out_path.to_string_lossy(),
    )?;
    write_json(&out_path, &benchmark)?;
    Ok(benchmark)
}

/// Add exactly one warning per task affected by the write guard. Boundary
/// blocks are included: whether legitimate or erroneous, each denial changed
/// the agent's available actions and therefore the observed eval behavior.
fn collect_guard_denial_warnings(iteration_dir: &Path, warnings: &mut Vec<String>) {
    let Ok(raw) = fs::read_to_string(iteration_dir.join("guard-denials.json")) else {
        return;
    };
    let Ok(report) = serde_json::from_str::<GuardDenialsReport>(&raw) else {
        return;
    };
    for task in report.tasks {
        let run = task
            .run_index
            .map(|index| format!("/run-{index}"))
            .unwrap_or_default();
        let denial_word = if task.denial_count == 1 {
            "denial"
        } else {
            "denials"
        };
        warnings.push(format!(
            "{}/{}{run} encountered {} guard {denial_word} — agent behavior changed; review \
             guard-denials.json before trusting this data point, even when the blocked boundary \
             was intentional.",
            task.eval_id, task.condition, task.denial_count
        ));
    }
}

/// The provenance label for a timing record (`completion-event` when absent).
fn timing_source_label(source: Option<TimingSource>) -> String {
    match source {
        Some(TimingSource::Transcript) => "transcript",
        Some(TimingSource::CompletionEvent) | None => "completion-event",
    }
    .to_string()
}

/// Add a warning per stray-write violation / live-source read. A malformed
/// report is ignored rather than failing aggregation — the warnings are
/// advisory, not a gate.
fn collect_stray_warnings(iteration_dir: &Path, warnings: &mut Vec<String>) {
    let Ok(raw) = fs::read_to_string(iteration_dir.join("stray-writes.json")) else {
        return;
    };
    let Ok(report) = serde_json::from_str::<StrayReport>(&raw) else {
        return;
    };
    for r in &report.runs {
        if !r.violations.is_empty() {
            warnings.push(format!(
                "{}/{} wrote {} file(s) outside its task environment — data point may be tainted (see stray-writes.json).",
                r.eval_id,
                r.condition,
                r.violations.len()
            ));
        }
        if !r.live_source_reads.is_empty() {
            warnings.push(format!(
                "{}/{} read the live skill source {} time(s) instead of its staged copy — the arm may be contaminated (staged-slug resolution race; see stray-writes.json).",
                r.eval_id,
                r.condition,
                r.live_source_reads.len()
            ));
        }
    }
}

/// Add plugin-shadow validity warnings. A malformed report is ignored.
fn collect_shadow_warnings(
    iteration_dir: &Path,
    conditions: &ConditionsRecord,
    warnings: &mut Vec<String>,
) {
    let Ok(raw) = fs::read_to_string(iteration_dir.join("plugin-shadow.json")) else {
        return;
    };
    let Ok(artifact) = serde_json::from_str::<PluginShadowArtifact>(&raw) else {
        return;
    };
    if artifact.isolates_live_sources {
        return;
    }
    let report = artifact.report;
    let rendered = conditions.harness.map_or_else(
        || shadow_validity_warnings(&report),
        |harness| adapter_for(harness).shadow_validity_warnings(&report),
    );
    warnings.extend(rendered);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_of_empty_is_zero() {
        assert_eq!(mean(&[]), 0.0);
    }

    #[test]
    fn mean_and_stddev() {
        let v = [1.0, 2.0, 3.0];
        assert_eq!(mean(&v), 2.0);
        // population stddev of [1,2,3] about 2 = sqrt(2/3)
        assert!((stddev(&v, 2.0) - (2.0f64 / 3.0).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn stddev_zero_for_fewer_than_two() {
        assert_eq!(stddev(&[5.0], 5.0), 0.0);
        assert_eq!(stddev(&[], 0.0), 0.0);
    }

    #[test]
    fn round_to_places() {
        assert_eq!(round(1.23456, 3), 1.235);
        assert_eq!(round(1999.6, 0), 2000.0);
    }

    #[test]
    fn stats_reports_n_and_rounds() {
        let s = stats(&[1.0, 1.0, 1.0], 3);
        assert_eq!(s.mean, 1.0);
        assert_eq!(s.stddev, 0.0);
        assert_eq!(s.n, 3);
    }

    #[test]
    fn timing_label_defaults_to_completion_event() {
        assert_eq!(timing_source_label(None), "completion-event");
        assert_eq!(
            timing_source_label(Some(TimingSource::Transcript)),
            "transcript"
        );
    }
}
