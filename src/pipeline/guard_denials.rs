//! Collection of per-task guard denial logs into the iteration artifact.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::pipeline::error::PipelineError;
use crate::pipeline::io::{now_iso8601, write_json};
use crate::sandbox::{GUARD_DENIALS_DIR, GUARD_DENIALS_LOG, GuardDenialRecord};
use crate::validation::{SchemaName, validate_against_schema};

/// Every raw denial associated with one dispatch task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TaskGuardDenials {
    pub eval_id: String,
    pub condition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_index: Option<u32>,
    pub denial_count: usize,
    pub denials: Vec<GuardDenialRecord>,
}

/// The iteration-level `guard-denials.json` report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GuardDenialsReport {
    pub generated: String,
    pub iteration: u32,
    pub total_denials: usize,
    pub tasks: Vec<TaskGuardDenials>,
}

#[derive(Debug, Deserialize)]
struct DispatchEnvelope {
    tasks: Option<Vec<DispatchRef>>,
}

#[derive(Debug, Deserialize)]
struct DispatchRef {
    eval_id: String,
    condition: String,
    #[serde(default)]
    run_index: Option<u32>,
    #[serde(default)]
    eval_root: Option<String>,
}

/// Join each task's raw denial JSONL through dispatch.json and write the
/// schema-gated iteration artifact. Collection is dispatch-driven, so denials
/// remain visible even when an agent never produced run.json.
pub(crate) fn collect_guard_denials(
    iteration_dir: &Path,
    iteration: u32,
    repo_root: &Path,
) -> Result<GuardDenialsReport, PipelineError> {
    let dispatch = std::fs::read_to_string(iteration_dir.join("dispatch.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<DispatchEnvelope>(&raw).ok());
    let mut tasks = Vec::new();

    for task in dispatch
        .and_then(|envelope| envelope.tasks)
        .unwrap_or_default()
    {
        let Some(eval_root) = task.eval_root else {
            continue;
        };
        let eval_root = PathBuf::from(eval_root);
        let eval_root = if eval_root.is_absolute() {
            eval_root
        } else {
            repo_root.join(eval_root)
        };
        let source_path = eval_root.join(GUARD_DENIALS_DIR).join(GUARD_DENIALS_LOG);
        let Ok(raw) = std::fs::read_to_string(&source_path) else {
            continue;
        };

        let mut denials = Vec::new();
        for (index, line) in raw.lines().enumerate() {
            let line_number = index + 1;
            let record = serde_json::from_str::<GuardDenialRecord>(line).map_err(|error| {
                PipelineError::Message(format!(
                    "{}:{line_number}: malformed guard denial record: {error}",
                    source_path.display()
                ))
            })?;
            denials.push(record);
        }
        if !denials.is_empty() {
            tasks.push(TaskGuardDenials {
                eval_id: task.eval_id,
                condition: task.condition,
                run_index: task.run_index,
                denial_count: denials.len(),
                denials,
            });
        }
    }

    tasks.sort_by(|a, b| {
        (&a.eval_id, &a.condition, a.run_index).cmp(&(&b.eval_id, &b.condition, b.run_index))
    });
    let total_denials = tasks.iter().map(|task| task.denial_count).sum();
    let report = GuardDenialsReport {
        generated: now_iso8601(),
        iteration,
        total_denials,
        tasks,
    };
    let out_path = iteration_dir.join("guard-denials.json");
    validate_against_schema::<serde_json::Value>(
        SchemaName::GuardDenials,
        &serde_json::to_value(&report)?,
        &out_path.to_string_lossy(),
    )?;
    write_json(&out_path, &report)?;
    Ok(report)
}
