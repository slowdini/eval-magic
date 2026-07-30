//! Collection of per-task permission denials from harness captures into the
//! iteration artifact.
//!
//! A harness can report a refused tool call while the dispatch still exits 0
//! and the run grades normally. This report preserves that otherwise easy-to-miss
//! signal.
//!
//! Guard blocks land here too: harnesses report the write guard's PreToolUse
//! denial like any other refusal. They are recorded but attributed, so
//! `aggregate` can leave them to `guard-denials.json` (which additionally carries
//! resolved targets) instead of warning twice about one denial.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::adapters::PermissionDenial;
use crate::pipeline::error::PipelineError;
use crate::pipeline::io::{now_iso8601, write_json};
use crate::sandbox::GUARD_REASON_PREFIX;
use crate::validation::{SchemaName, validate_against_schema};

/// One refused tool call, plus where the refusal came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PermissionDenialRecord {
    #[serde(flatten)]
    pub denial: PermissionDenial,
    /// True when the reason identifies this as an eval write-guard block, which
    /// `guard-denials.json` already reports in full.
    pub guard_attributed: bool,
}

/// Every refused tool call associated with one dispatch task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TaskPermissionDenials {
    pub eval_id: String,
    pub condition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_index: Option<u32>,
    pub denial_count: usize,
    pub guard_attributed_count: usize,
    pub denials: Vec<PermissionDenialRecord>,
}

/// The iteration-level `permission-denials.json` report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PermissionDenialsReport {
    pub generated: String,
    pub iteration: u32,
    pub total_denials: usize,
    pub tasks: Vec<TaskPermissionDenials>,
}

impl TaskPermissionDenials {
    /// Attribute each denial and tally the task.
    pub(crate) fn new(
        eval_id: String,
        condition: String,
        run_index: Option<u32>,
        denials: Vec<PermissionDenial>,
    ) -> Self {
        let denials: Vec<PermissionDenialRecord> = denials
            .into_iter()
            .map(|denial| {
                let guard_attributed = denial
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.starts_with(GUARD_REASON_PREFIX));
                PermissionDenialRecord {
                    denial,
                    guard_attributed,
                }
            })
            .collect();
        let guard_attributed_count = denials
            .iter()
            .filter(|record| record.guard_attributed)
            .count();
        Self {
            eval_id,
            condition,
            run_index,
            denial_count: denials.len(),
            guard_attributed_count,
            denials,
        }
    }

    /// Refusals this report owns: the harness's own, excluding what the write
    /// guard blocked. Zero means the guard warning already covers this task.
    pub(crate) fn harness_denial_count(&self) -> usize {
        self.denial_count - self.guard_attributed_count
    }
}

/// Write the schema-gated iteration artifact. Tasks with no denials are dropped,
/// and the rest are sorted by `(eval_id, condition, run_index)` — `aggregate`
/// emits warnings in report order and never sorts them, so determinism has to
/// come from here.
pub(crate) fn write_report(
    iteration_dir: &Path,
    iteration: u32,
    tasks: Vec<TaskPermissionDenials>,
) -> Result<PermissionDenialsReport, PipelineError> {
    let mut tasks: Vec<TaskPermissionDenials> = tasks
        .into_iter()
        .filter(|task| task.denial_count > 0)
        .collect();
    tasks.sort_by(|a, b| {
        (&a.eval_id, &a.condition, a.run_index).cmp(&(&b.eval_id, &b.condition, b.run_index))
    });
    let total_denials = tasks.iter().map(|task| task.denial_count).sum();
    let report = PermissionDenialsReport {
        generated: now_iso8601(),
        iteration,
        total_denials,
        tasks,
    };
    let out_path = iteration_dir.join("permission-denials.json");
    validate_against_schema::<serde_json::Value>(
        SchemaName::PermissionDenials,
        &serde_json::to_value(&report)?,
        &out_path.to_string_lossy(),
    )?;
    write_json(&out_path, &report)?;
    Ok(report)
}

/// Add one warning per task whose harness refused a tool call. Guard-attributed
/// denials are skipped: the guard blocks through the same permission mechanism,
/// so warning here as well would double-count one denial and bury the refusals
/// only this report can see. A task with nothing but guard blocks therefore emits
/// nothing, and the guard-denial warning covers it with better detail.
pub(super) fn collect_warnings(iteration_dir: &Path, warnings: &mut Vec<String>) {
    let Ok(raw) = fs::read_to_string(iteration_dir.join("permission-denials.json")) else {
        return;
    };
    let Ok(report) = serde_json::from_str::<PermissionDenialsReport>(&raw) else {
        return;
    };
    for task in report.tasks {
        let refused = task.harness_denial_count();
        if refused == 0 {
            continue;
        }
        let run = task
            .run_index
            .map(|index| format!("/run-{index}"))
            .unwrap_or_default();
        let call_word = if refused == 1 { "call" } else { "calls" };
        warnings.push(format!(
            "{}/{}{run} had {refused} permission-denied tool {call_word} — the refused attempt \
             changed agent behavior and often degrades the run to static reasoning; review \
             permission-denials.json before trusting this data point.",
            task.eval_id, task.condition
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::PermissionDenial;
    use tempfile::TempDir;

    fn denial(tool: &str, reason: &str) -> PermissionDenial {
        PermissionDenial {
            tool: tool.to_string(),
            reason: Some(reason.to_string()),
            input_keys: vec!["command".to_string()],
        }
    }

    #[test]
    fn guard_blocks_are_attributed_and_left_out_of_the_harness_denial_count() {
        let task = TaskPermissionDenials::new(
            "tz-bug".to_string(),
            "with_skill".to_string(),
            None,
            vec![
                denial("Bash", "This command requires approval"),
                denial(
                    "Write",
                    "eval guard: Write to /tmp/x is outside the eval sandbox",
                ),
            ],
        );

        // Every denial is recorded; only the non-guard one is the pipeline's to
        // report, because guard-denials.json already covers the other.
        assert_eq!(task.denials.len(), 2);
        assert_eq!(task.denial_count, 2);
        assert_eq!(task.guard_attributed_count, 1);
        assert_eq!(task.harness_denial_count(), 1);
        assert!(!task.denials[0].guard_attributed);
        assert!(task.denials[1].guard_attributed);
    }

    #[test]
    fn a_denial_without_a_reason_is_not_attributed_to_the_guard() {
        let task = TaskPermissionDenials::new(
            "tz-bug".to_string(),
            "with_skill".to_string(),
            Some(2),
            vec![PermissionDenial {
                tool: "Bash".to_string(),
                reason: None,
                input_keys: Vec::new(),
            }],
        );
        assert_eq!(task.guard_attributed_count, 0);
        assert_eq!(task.harness_denial_count(), 1);
    }

    #[test]
    fn a_real_guard_verdict_is_attributed_to_the_guard() {
        // Pins the two ends together: whatever wording the arbiter produces, the
        // collector must recognize it as the guard's own block.
        let marker = crate::sandbox::GuardMarker {
            active: Some(true),
            allowed_roots: Some(vec!["/env".to_string()]),
            expires_at: None,
            denial_log_path: None,
        };
        let verdict = crate::sandbox::decide(
            "Write",
            &serde_json::json!({"file_path": "/etc/passwd"}),
            Some(&marker),
            0,
        );
        assert!(!verdict.allow);

        let task = TaskPermissionDenials::new(
            "tz-bug".to_string(),
            "with_skill".to_string(),
            None,
            vec![PermissionDenial {
                tool: "Write".to_string(),
                reason: verdict.reason,
                input_keys: vec!["file_path".to_string()],
            }],
        );
        assert_eq!(task.guard_attributed_count, 1);
        assert_eq!(task.harness_denial_count(), 0);
    }

    #[test]
    fn report_sorts_tasks_and_totals_every_denial() {
        let dir = TempDir::new().unwrap();
        let tasks = vec![
            TaskPermissionDenials::new(
                "b-eval".to_string(),
                "with_skill".to_string(),
                None,
                vec![denial("Bash", "This command requires approval")],
            ),
            TaskPermissionDenials::new(
                "a-eval".to_string(),
                "without_skill".to_string(),
                Some(2),
                vec![denial("Bash", "This command requires approval")],
            ),
            TaskPermissionDenials::new(
                "a-eval".to_string(),
                "without_skill".to_string(),
                Some(1),
                vec![
                    denial("Bash", "This command requires approval"),
                    denial("Bash", "eval guard: blocked Bash (cd /) — runs outside"),
                ],
            ),
        ];

        let report = write_report(dir.path(), 1, tasks).unwrap();
        assert_eq!(report.total_denials, 4);
        let order: Vec<(&str, Option<u32>)> = report
            .tasks
            .iter()
            .map(|t| (t.eval_id.as_str(), t.run_index))
            .collect();
        assert_eq!(
            order,
            vec![("a-eval", Some(1)), ("a-eval", Some(2)), ("b-eval", None)]
        );

        // Written where aggregate looks for it, and schema-valid.
        let raw = std::fs::read_to_string(dir.path().join("permission-denials.json")).unwrap();
        let parsed: PermissionDenialsReport = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.total_denials, 4);
        assert_eq!(parsed.iteration, 1);
    }

    #[test]
    fn a_task_with_no_denials_is_omitted_from_the_report() {
        let dir = TempDir::new().unwrap();
        let report = write_report(
            dir.path(),
            1,
            vec![TaskPermissionDenials::new(
                "tz-bug".to_string(),
                "with_skill".to_string(),
                None,
                Vec::new(),
            )],
        )
        .unwrap();
        assert!(report.tasks.is_empty());
        assert_eq!(report.total_denials, 0);
    }
}
