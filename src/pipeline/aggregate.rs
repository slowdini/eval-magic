//! Stage 5 — `aggregate`.
//!
//! Ports `src/pipeline/aggregate.ts`. Compares exactly two conditions: collects
//! `pass_rate` (from `grading.json`), `total_tokens`/`duration_ms` (from
//! `timing.json`), and the skill-invocation determination per condition; computes
//! mean/stddev and the `a - b` delta; accumulates validity warnings (mixed timing
//! sources, sub-100% invocation rate, stray-write violations + live-source reads,
//! plugin shadows); and writes `benchmark.json`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::adapters::{PluginShadowReport, shadow_validity_warnings};
use crate::core::{ConditionsRecord, GradingResult, Mode, TimingRecord, TimingSource};
use crate::pipeline::error::PipelineError;
use crate::pipeline::io::{now_iso8601, write_json};

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
    delta: Delta,
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

    for eval_dir in &eval_dirs {
        for cond in &condition_names {
            let cond_dir = iteration_dir.join(eval_dir).join(cond);
            let grading_path = cond_dir.join("grading.json");
            let timing_path = cond_dir.join("timing.json");

            if !grading_path.exists() {
                eprintln!("warn: missing grading for {eval_dir}/{cond}");
                missing_gradings += 1;
                continue;
            }
            let grading: GradingResult = serde_json::from_str(&fs::read_to_string(&grading_path)?)?;
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
            "runs mix timing sources ({joined}) — transcript-derived totals include cache \
             accounting, so the token/duration delta compares two different metrics. Re-record \
             one side or read the delta as a rough signal only."
        ));
    }
    for cond in &condition_names {
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

    collect_stray_warnings(iteration_dir, &mut validity_warnings);
    collect_shadow_warnings(iteration_dir, &mut validity_warnings);

    let benchmark = Benchmark {
        generated: now_iso8601(),
        mode: conditions.mode,
        baseline: conditions.baseline.clone(),
        conditions_compared: vec![a.clone(), b.clone()],
        missing_gradings,
        validity_warnings,
        run_summary: Value::Object(run_summary),
        delta,
    };

    write_json(&iteration_dir.join("benchmark.json"), &benchmark)?;
    Ok(benchmark)
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
/// report is ignored rather than failing aggregation (mirrors the TS try/catch).
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
                "{}/{} wrote {} file(s) outside its outputs dir — data point may be tainted (see stray-writes.json).",
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
fn collect_shadow_warnings(iteration_dir: &Path, warnings: &mut Vec<String>) {
    let Ok(raw) = fs::read_to_string(iteration_dir.join("plugin-shadow.json")) else {
        return;
    };
    let Ok(report) = serde_json::from_str::<PluginShadowReport>(&raw) else {
        return;
    };
    warnings.extend(shadow_validity_warnings(&report));
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
