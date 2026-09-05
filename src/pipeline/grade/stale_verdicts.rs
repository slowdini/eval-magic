//! Reporting judge verdicts that answer an assertion's previous definition.
//!
//! Verdicts are cached by response path, so a reworded rubric under an
//! unchanged assertion id would be finalized from the verdict on the old
//! rubric. Assertions come from the live `evals.json` and are expected to be
//! edited between runs, so a verdict that answers a previous definition is
//! reported rather than passed off as an answer to the current one.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use super::judge_tasks::JudgeTask;

/// What a previous emit recorded for each verdict file: the rubric a judge was
/// asked, and the model asked to answer it.
pub type EmittedDefinitions = HashMap<String, (String, Option<String>)>;

/// Read the definitions a previous `judge-tasks.json` recorded. Absent or
/// unreadable means no previous emit to compare against, never a mismatch.
pub fn emitted_definitions(tasks_path: &Path) -> EmittedDefinitions {
    let Ok(raw) = std::fs::read_to_string(tasks_path) else {
        return EmittedDefinitions::new();
    };
    let Ok(file) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return EmittedDefinitions::new();
    };
    file.get("tasks")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|task| {
            Some((
                task.get("response_path")?.as_str()?.to_string(),
                (
                    task.get("rubric")?.as_str()?.to_string(),
                    task.get("model")
                        .and_then(|model| model.as_str().map(std::string::ToString::to_string)),
                ),
            ))
        })
        .collect()
}

/// One warning naming every assertion whose definition changed since the
/// verdict on disk answered it.
///
/// Collected across the whole emit so an edited assertion is named once, not
/// once per `(condition, run, sample)` cell it appears in.
pub fn warning(previous: &EmittedDefinitions, tasks: &[JudgeTask]) -> Option<String> {
    let stale: BTreeSet<String> = tasks
        .iter()
        .filter(|task| {
            previous
                .get(&task.response_path)
                .is_some_and(|(rubric, model)| {
                    (rubric, model) != (&task.rubric, &task.model)
                        && Path::new(&task.response_path).exists()
                })
        })
        .map(|task| format!("{}/{}", task.eval_id, task.assertion_id))
        .collect();
    if stale.is_empty() {
        return None;
    }
    Some(format!(
        "{} assertion(s) changed since their judge verdict was written: {}. Those verdicts answer the previous rubric and are reused as-is — re-dispatch judges with --overwrite to re-judge them.",
        stale.len(),
        stale.into_iter().collect::<Vec<_>>().join(", ")
    ))
}
