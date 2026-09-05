//! Per-assertion counts for `benchmark.json`.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::core::{GradedAssertionResult, SampledAssertionResult};
use crate::pipeline::error::PipelineError;

#[derive(Debug)]
enum AssertionCount {
    Binary {
        passed: u32,
        n: u32,
    },
    Sampled {
        passed: u32,
        total: u32,
        samples_per_run: u32,
        run_count: u32,
    },
}

type Counts = HashMap<String, HashMap<String, HashMap<String, AssertionCount>>>;

#[derive(Debug, Default)]
pub(super) struct AssertionRollup {
    counts: Counts,
}

impl AssertionRollup {
    pub(super) fn record(
        &mut self,
        eval_id: &str,
        condition: &str,
        results: &[GradedAssertionResult],
    ) -> Result<(), PipelineError> {
        let eval_counts = self.counts.entry(eval_id.to_string()).or_default();
        for result in results {
            let condition_counts = eval_counts
                .entry(result.id().to_string())
                .or_default()
                .entry(condition.to_string());
            match result {
                GradedAssertionResult::Binary(result) => {
                    let count =
                        condition_counts.or_insert(AssertionCount::Binary { passed: 0, n: 0 });
                    let AssertionCount::Binary { passed, n } = count else {
                        return Err(inconsistent_shape(eval_id, result.id.as_str(), condition));
                    };
                    *n += 1;
                    if result.passed {
                        *passed += 1;
                    }
                }
                GradedAssertionResult::Sampled(SampledAssertionResult { votes, .. }) => {
                    let count = condition_counts.or_insert(AssertionCount::Sampled {
                        passed: 0,
                        total: 0,
                        samples_per_run: votes.total,
                        run_count: 0,
                    });
                    let AssertionCount::Sampled {
                        passed,
                        total,
                        samples_per_run,
                        run_count,
                    } = count
                    else {
                        return Err(inconsistent_shape(eval_id, result.id(), condition));
                    };
                    if *samples_per_run != votes.total {
                        return Err(PipelineError::Message(format!(
                            "inconsistent judge sample counts for {eval_id}/{}/{condition}: expected {samples_per_run}, found {}",
                            result.id(),
                            votes.total
                        )));
                    }
                    *passed += votes.passed;
                    *total += votes.total;
                    *run_count += 1;
                }
            }
        }
        Ok(())
    }

    /// Render stable eval/assertion ordering while preserving the declared
    /// condition order from `conditions.json`.
    pub(super) fn into_value(self, eval_dirs: &[String], condition_names: &[String]) -> Value {
        let mut by_eval = Map::new();
        for eval_dir in eval_dirs {
            let eval_id = eval_dir.strip_prefix("eval-").unwrap_or(eval_dir);
            let Some(eval_counts) = self.counts.get(eval_id) else {
                continue;
            };
            let mut assertion_ids: Vec<&String> = eval_counts.keys().collect();
            assertion_ids.sort();

            let mut by_assertion = Map::new();
            for assertion_id in assertion_ids {
                let mut by_condition = Map::new();
                for condition in condition_names {
                    if let Some(count) = eval_counts[assertion_id].get(condition) {
                        by_condition.insert(condition.clone(), count.to_value());
                    }
                }
                by_assertion.insert(assertion_id.clone(), Value::Object(by_condition));
            }
            by_eval.insert(eval_id.to_string(), Value::Object(by_assertion));
        }
        Value::Object(by_eval)
    }
}

impl AssertionCount {
    fn to_value(&self) -> Value {
        match self {
            Self::Binary { passed, n } => json!({ "passed": passed, "n": n }),
            Self::Sampled {
                passed,
                total,
                samples_per_run,
                run_count,
            } => {
                let proportion = f64::from(*passed) / f64::from(*total);
                json!({
                    "votes": {
                        "passed": passed,
                        "failed": total - passed,
                        "total": total,
                        "proportion": proportion
                    },
                    "samples_per_run": samples_per_run,
                    "run_count": run_count,
                    "pass_power_k": proportion.powf(f64::from(*samples_per_run))
                })
            }
        }
    }
}

fn inconsistent_shape(eval_id: &str, assertion_id: &str, condition: &str) -> PipelineError {
    PipelineError::Message(format!(
        "inconsistent grading shapes for {eval_id}/{assertion_id}/{condition}: cannot combine binary and sampled assertion results"
    ))
}
