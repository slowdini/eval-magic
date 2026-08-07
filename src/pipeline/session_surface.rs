//! Collection of the per-dispatch skill/plugin surface into the iteration
//! artifact.
//!
//! The shadow preflight runs before anything dispatches, so it can only report
//! what is *discoverable* from an environment. What a dispatch actually loaded is
//! knowable only afterwards, from its own transcript. This report carries that
//! evidence so `aggregate` can tell a real collision from a correctly-isolated
//! run instead of warning about both.
//!
//! Only written when the harness can report a surface, so a missing file always
//! means "cannot report" rather than "nothing loaded". Within a task, evidence is
//! per round: isolation has to hold for the initial dispatch *and* every resumed
//! turn, so one round without evidence leaves the whole task unproven.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::adapters::SessionSurface;
use crate::core::fs::write_json;
use crate::pipeline::error::PipelineError;
use crate::pipeline::io::now_iso8601;
use crate::validation::{SchemaName, validate_against_schema};

/// One CLI invocation's reported surface. `None` means that round's transcript
/// reported nothing knowable — never that nothing loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RoundSurface {
    /// 1-based round: `1` for a one-shot dispatch, then one per resumed turn.
    pub round: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<SessionSurface>,
}

/// Every round's surface for one dispatch task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TaskSessionSurface {
    pub eval_id: String,
    pub condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_index: Option<u32>,
    /// Group this task belongs to; absent for a single-group run, matching
    /// `dispatch.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub rounds: Vec<RoundSurface>,
}

/// The iteration-level `session-surface.json` report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SessionSurfaceReport {
    pub generated: String,
    pub iteration: u32,
    pub tasks_with_evidence: usize,
    pub tasks_without_evidence: usize,
    pub tasks: Vec<TaskSessionSurface>,
}

impl TaskSessionSurface {
    /// Whether this task proves what it loaded. Requires at least one round and
    /// a surface for *every* round: a task whose second turn reported nothing
    /// cannot show that the isolation held for that turn, and a task with no
    /// rounds at all never ran.
    pub(crate) fn has_evidence(&self) -> bool {
        !self.rounds.is_empty() && self.rounds.iter().all(|round| round.surface.is_some())
    }

    /// Whether any round advertised `runtime_id`.
    pub(crate) fn advertises(&self, runtime_id: &str) -> bool {
        self.rounds
            .iter()
            .filter_map(|r| r.surface.as_ref())
            .any(|surface: &SessionSurface| {
                surface
                    .advertised_skills
                    .iter()
                    .any(|advertised| advertised == runtime_id)
            })
    }

    /// Whether any round loaded the plugin identified by `plugin_key` (the
    /// `enabledPlugins` key, e.g. `slow-powers@slowdini`). Matches the reported
    /// `source` when the harness gives one, else the bare name before `@`.
    pub(crate) fn loaded_plugin(&self, plugin_key: &str) -> bool {
        let bare = plugin_key.split_once('@').map_or(plugin_key, |(n, _)| n);
        self.rounds
            .iter()
            .filter_map(|r| r.surface.as_ref())
            .flat_map(|surface| &surface.loaded_plugins)
            .any(|plugin| match plugin.source.as_deref() {
                Some(source) => source == plugin_key,
                None => plugin.name == bare,
            })
    }
}

impl SessionSurfaceReport {
    /// Every dispatch recorded for one comparison cell's `(eval_id, condition)`.
    pub(crate) fn tasks_for(&self, eval_id: &str, condition: &str) -> Vec<&TaskSessionSurface> {
        self.tasks
            .iter()
            .filter(|task| task.eval_id == eval_id && task.condition == condition)
            .collect()
    }
}

/// Write the schema-gated iteration artifact. Tasks are sorted by
/// `(eval_id, condition, run_index)` so downstream warning order is
/// deterministic. Unlike the denial report, tasks without evidence are *kept*:
/// their absence is the signal that a finding cannot be verified.
pub(crate) fn write_report(
    iteration_dir: &Path,
    iteration: u32,
    tasks: Vec<TaskSessionSurface>,
) -> Result<SessionSurfaceReport, PipelineError> {
    let mut tasks = tasks;
    tasks.sort_by(|a, b| {
        (&a.eval_id, &a.condition, a.run_index).cmp(&(&b.eval_id, &b.condition, b.run_index))
    });
    let tasks_with_evidence = tasks.iter().filter(|task| task.has_evidence()).count();
    let report = SessionSurfaceReport {
        generated: now_iso8601(),
        iteration,
        tasks_with_evidence,
        tasks_without_evidence: tasks.len() - tasks_with_evidence,
        tasks,
    };
    let out_path = iteration_dir.join("session-surface.json");
    validate_against_schema::<serde_json::Value>(
        SchemaName::SessionSurface,
        &serde_json::to_value(&report)?,
        &out_path.to_string_lossy(),
    )?;
    write_json(&out_path, &report)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::LoadedPlugin;
    use tempfile::TempDir;

    fn surface(skills: &[&str], plugins: &[(&str, Option<&str>)]) -> SessionSurface {
        SessionSurface {
            advertised_skills: skills.iter().map(|s| (*s).to_string()).collect(),
            loaded_plugins: plugins
                .iter()
                .map(|(name, source)| LoadedPlugin {
                    name: (*name).to_string(),
                    source: source.map(str::to_string),
                    version: None,
                })
                .collect(),
        }
    }

    fn task(rounds: Vec<RoundSurface>) -> TaskSessionSurface {
        TaskSessionSurface {
            eval_id: "e1".into(),
            condition: "with_skill".into(),
            run_index: None,
            group: Some("g1".into()),
            rounds,
        }
    }

    #[test]
    fn an_empty_surface_is_evidence_but_a_missing_one_is_not() {
        let reported = task(vec![RoundSurface {
            round: 1,
            surface: Some(surface(&[], &[])),
        }]);
        assert!(reported.has_evidence());

        let silent = task(vec![RoundSurface {
            round: 1,
            surface: None,
        }]);
        assert!(!silent.has_evidence());

        assert!(!task(vec![]).has_evidence(), "a task that never ran");
    }

    #[test]
    fn a_scripted_task_needs_every_round_reported_to_count_as_evidence() {
        // Isolation must hold for the resumed turns too, so one silent round
        // leaves the whole task unproven rather than partially proven.
        let partial = task(vec![
            RoundSurface {
                round: 1,
                surface: Some(surface(&[], &[])),
            },
            RoundSurface {
                round: 2,
                surface: None,
            },
        ]);
        assert!(!partial.has_evidence());
    }

    #[test]
    fn a_plugin_matches_on_its_enabled_plugins_key_or_bare_name() {
        let with_source = task(vec![RoundSurface {
            round: 1,
            surface: Some(surface(
                &[],
                &[("slow-powers", Some("slow-powers@slowdini"))],
            )),
        }]);
        assert!(with_source.loaded_plugin("slow-powers@slowdini"));
        assert!(!with_source.loaded_plugin("other@marketplace"));

        // A harness that reports only a name still matches the key's namespace.
        let name_only = task(vec![RoundSurface {
            round: 1,
            surface: Some(surface(&[], &[("slow-powers", None)])),
        }]);
        assert!(name_only.loaded_plugin("slow-powers@slowdini"));
        assert!(!name_only.loaded_plugin("unrelated@slowdini"));
    }

    #[test]
    fn advertised_skills_match_the_runtime_id_exactly() {
        let loaded = task(vec![RoundSurface {
            round: 1,
            surface: Some(surface(&["slow-powers:hardening-plans"], &[])),
        }]);
        assert!(loaded.advertises("slow-powers:hardening-plans"));
        // The bare logical name is a different runtime id and must not match.
        assert!(!loaded.advertises("hardening-plans"));
    }

    #[test]
    fn a_later_round_advertising_the_skill_counts_for_the_task() {
        let contaminated = task(vec![
            RoundSurface {
                round: 1,
                surface: Some(surface(&[], &[])),
            },
            RoundSurface {
                round: 2,
                surface: Some(surface(&["slow-powers:hardening-plans"], &[])),
            },
        ]);
        assert!(contaminated.advertises("slow-powers:hardening-plans"));
    }

    #[test]
    fn write_report_sorts_tasks_and_tallies_evidence() {
        let dir = TempDir::new().unwrap();
        let with_surface =
            |eval_id: &str, condition: &str, run_index: Option<u32>, reported: bool| {
                TaskSessionSurface {
                    eval_id: eval_id.into(),
                    condition: condition.into(),
                    run_index,
                    group: Some("g1".into()),
                    rounds: vec![RoundSurface {
                        round: 1,
                        surface: reported.then(|| surface(&[], &[])),
                    }],
                }
            };
        let report = write_report(
            dir.path(),
            2,
            vec![
                with_surface("b-eval", "with_skill", Some(1), true),
                with_surface("a-eval", "without_skill", None, false),
                with_surface("a-eval", "with_skill", None, true),
            ],
        )
        .unwrap();

        assert_eq!(report.iteration, 2);
        assert_eq!(report.tasks_with_evidence, 2);
        assert_eq!(report.tasks_without_evidence, 1);
        let order: Vec<(&str, &str)> = report
            .tasks
            .iter()
            .map(|task| (task.eval_id.as_str(), task.condition.as_str()))
            .collect();
        assert_eq!(
            order,
            [
                ("a-eval", "with_skill"),
                ("a-eval", "without_skill"),
                ("b-eval", "with_skill")
            ]
        );
        assert!(dir.path().join("session-surface.json").exists());
    }

    #[test]
    fn tasks_for_selects_only_the_matching_cell() {
        let report = SessionSurfaceReport {
            generated: "2026-08-07T00:00:00Z".into(),
            iteration: 1,
            tasks_with_evidence: 0,
            tasks_without_evidence: 0,
            tasks: vec![
                TaskSessionSurface {
                    eval_id: "e1".into(),
                    condition: "with_skill".into(),
                    run_index: Some(1),
                    group: Some("g1".into()),
                    rounds: vec![],
                },
                TaskSessionSurface {
                    eval_id: "e1".into(),
                    condition: "without_skill".into(),
                    run_index: Some(1),
                    group: Some("g1".into()),
                    rounds: vec![],
                },
            ],
        };
        assert_eq!(report.tasks_for("e1", "with_skill").len(), 1);
        assert_eq!(report.tasks_for("e1", "missing").len(), 0);
    }
}
