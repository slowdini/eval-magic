//! Per-assertion counts for `benchmark.json`.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::core::AssertionResult;

#[derive(Debug, Default, Serialize)]
struct AssertionCount {
    passed: usize,
    n: usize,
}

type Counts = HashMap<String, HashMap<String, HashMap<String, AssertionCount>>>;

#[derive(Debug, Default)]
pub(super) struct AssertionRollup {
    counts: Counts,
}

impl AssertionRollup {
    pub(super) fn record(&mut self, eval_id: &str, condition: &str, results: &[AssertionResult]) {
        let eval_counts = self.counts.entry(eval_id.to_string()).or_default();
        for result in results {
            let count = eval_counts
                .entry(result.id.clone())
                .or_default()
                .entry(condition.to_string())
                .or_default();
            count.n += 1;
            if result.passed {
                count.passed += 1;
            }
        }
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
                        by_condition.insert(
                            condition.clone(),
                            serde_json::to_value(count).expect("assertion counts serialize"),
                        );
                    }
                }
                by_assertion.insert(assertion_id.clone(), Value::Object(by_condition));
            }
            by_eval.insert(eval_id.to_string(), Value::Object(by_assertion));
        }
        Value::Object(by_eval)
    }
}
