//! Stage 2 — `fill-transcripts`.
//!
//! Walks the iteration's `eval-*`
//! directories and, for each `(eval, condition)` `run.json`, populates
//! `tool_invocations` from the persisted transcript (Claude Code subagent JSONL
//! resolved by the task's `agent_description`, or Codex `codex-events.jsonl`).
//! Records that already carry invocations are skipped unless `overwrite`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::adapters::{adapter_for, find_by_description};
use crate::core::{
    ConditionsRecord, DispatchMechanism, Harness, RunRecord, ToolInvocation, mechanism_for,
};
use crate::pipeline::error::PipelineError;
use crate::pipeline::io::write_json;
use crate::pipeline::slots::{run_key, run_slots};
use crate::validation::{SchemaName, validate_against_schema};

/// Tally of what fill-transcripts did across the iteration's runs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FillTranscriptsResult {
    pub filled: usize,
    pub skipped: usize,
    pub missing: usize,
}

/// The `dispatch.json` fields fill-transcripts reads back.
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
    agent_description: Option<String>,
    #[serde(default)]
    outputs_dir: Option<String>,
}

/// The canonical dispatch description for an `(eval, condition, run)` run.
///
/// The runner writes a unique `agent_description` per task into `dispatch.json`
/// (namespaced with the iteration + run nonce); reading it back binds each run to
/// the exact agent that produced it. Falls back to the
/// `<eval_id>:<condition>[:r<k>]` reconstruction when `dispatch.json` is absent,
/// malformed, or missing the task (hand-authored/operator runs).
pub fn resolve_agent_description(
    iteration_dir: &Path,
    eval_id: &str,
    condition: &str,
    run_index: Option<u32>,
) -> String {
    let dispatch_path = iteration_dir.join("dispatch.json");
    if let Ok(raw) = fs::read_to_string(&dispatch_path)
        && let Ok(env) = serde_json::from_str::<DispatchEnvelope>(&raw)
        && let Some(tasks) = env.tasks
        && let Some(task) = tasks
            .iter()
            .find(|t| t.eval_id == eval_id && t.condition == condition && t.run_index == run_index)
        && let Some(desc) = &task.agent_description
    {
        return desc.clone();
    }
    run_key(eval_id, condition, run_index)
}

/// Populate `tool_invocations` for every `run.json` under `iteration_dir`. See
/// the module docs for the transcript sources and overwrite semantics.
pub fn fill_transcripts(
    iteration_dir: &Path,
    harness: Harness,
    subagents_dir: Option<&Path>,
    overwrite: bool,
) -> Result<FillTranscriptsResult, PipelineError> {
    let conditions_path = iteration_dir.join("conditions.json");
    if !conditions_path.exists() {
        return Err(PipelineError::Message(format!(
            "missing: {}",
            conditions_path.display()
        )));
    }
    let conditions: ConditionsRecord =
        serde_json::from_str(&fs::read_to_string(&conditions_path)?)?;
    let condition_names: Vec<String> = conditions
        .conditions
        .iter()
        .map(|c| c.name.clone())
        .collect();

    let outputs_by_key = outputs_dirs_by_key(iteration_dir);

    let mut result = FillTranscriptsResult::default();
    for entry in fs::read_dir(iteration_dir)? {
        let entry = entry?;
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let Some(eval_id) = dir_name.strip_prefix("eval-") else {
            continue;
        };

        for cond in &condition_names {
            let cond_dir = iteration_dir.join(&dir_name).join(cond);
            for slot in run_slots(&cond_dir) {
                let run_path = slot.dir.join("run.json");
                if !run_path.exists() {
                    continue;
                }

                let source = run_path.to_string_lossy();
                let mut run: RunRecord = validate_against_schema(
                    SchemaName::RunRecord,
                    &serde_json::from_str(&fs::read_to_string(&run_path)?)?,
                    &source,
                )?;

                if !run.tool_invocations.is_empty() && !overwrite {
                    result.skipped += 1;
                    continue;
                }

                let outputs_dir = outputs_by_key
                    .get(&run_key(eval_id, cond, slot.run_index))
                    .cloned()
                    .unwrap_or_else(|| slot.dir.join("outputs").to_string_lossy().into_owned());

                let Some(invocations) = invocations_for_run(
                    harness,
                    subagents_dir,
                    iteration_dir,
                    eval_id,
                    cond,
                    slot.run_index,
                    Path::new(&outputs_dir),
                ) else {
                    result.missing += 1;
                    continue;
                };

                run.tool_invocations = invocations;
                write_json(&run_path, &run)?;
                result.filled += 1;
            }
        }
    }

    Ok(result)
}

/// Map `"<eval_id>:<condition>[:r<k>]"` → the task's `outputs_dir` from
/// `dispatch.json`. Empty when the file is absent or malformed (callers fall
/// back to convention).
fn outputs_dirs_by_key(iteration_dir: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Ok(raw) = fs::read_to_string(iteration_dir.join("dispatch.json"))
        && let Ok(env) = serde_json::from_str::<DispatchEnvelope>(&raw)
    {
        for t in env.tasks.unwrap_or_default() {
            if let Some(dir) = t.outputs_dir {
                out.insert(run_key(&t.eval_id, &t.condition, t.run_index), dir);
            }
        }
    }
    out
}

/// Parse the invocations for one run, keyed on the dispatch mechanism: a
/// `Cli`-mechanism harness reads the events file its CLI wrote under
/// `outputs_dir` (e.g. Codex's `codex-events.jsonl`); an `InSession` harness
/// reads the subagent transcript matched by the resolved description.
fn invocations_for_run(
    harness: Harness,
    subagents_dir: Option<&Path>,
    iteration_dir: &Path,
    eval_id: &str,
    condition: &str,
    run_index: Option<u32>,
    outputs_dir: &Path,
) -> Option<Vec<ToolInvocation>> {
    match mechanism_for(harness) {
        DispatchMechanism::Cli => {
            let events_path = outputs_dir.join(adapter_for(harness).cli_events_filename()?);
            if !events_path.exists() {
                return None;
            }
            adapter_for(harness).parse_transcript(&events_path).ok()
        }
        DispatchMechanism::InSession => {
            let description =
                resolve_agent_description(iteration_dir, eval_id, condition, run_index);
            let subagent =
                find_by_description(subagents_dir.unwrap_or_else(|| Path::new("")), &description)?;
            adapter_for(harness)
                .parse_transcript(&subagent.jsonl_path)
                .ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_dispatch(iteration_dir: &Path, tasks: Value) {
        fs::create_dir_all(iteration_dir).unwrap();
        fs::write(
            iteration_dir.join("dispatch.json"),
            serde_json::to_string_pretty(&json!({"run_nonce": "abc123", "tasks": tasks})).unwrap(),
        )
        .unwrap();
    }

    fn jsonl(lines: &[Value]) -> String {
        let body = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        format!("{body}\n")
    }

    fn write_run_record(path: &Path, tool_invocations: Value) {
        let record = json!({
            "eval_id": "crash",
            "condition": "with_skill",
            "skill_path": "/skill/SKILL.md",
            "prompt": "Fix it",
            "files": [],
            "final_message": "Done.",
            "tool_invocations": tool_invocations,
            "total_tokens": Value::Null,
            "duration_ms": Value::Null,
        });
        fs::write(path, serde_json::to_string_pretty(&record).unwrap()).unwrap();
    }

    // --- resolveAgentDescription ---

    #[test]
    fn returns_the_namespaced_agent_description_from_dispatch() {
        let root = TempDir::new().unwrap();
        let dir = root.path().join("iter-canonical");
        write_dispatch(
            &dir,
            json!([
                {"eval_id": "crash", "condition": "with_skill", "agent_description": "crash:with_skill:i3-abc123"},
                {"eval_id": "crash", "condition": "without_skill", "agent_description": "crash:without_skill:i3-abc123"}
            ]),
        );
        assert_eq!(
            resolve_agent_description(&dir, "crash", "with_skill", None),
            "crash:with_skill:i3-abc123"
        );
        assert_eq!(
            resolve_agent_description(&dir, "crash", "without_skill", None),
            "crash:without_skill:i3-abc123"
        );
    }

    #[test]
    fn falls_back_to_legacy_reconstruction_when_dispatch_absent() {
        let root = TempDir::new().unwrap();
        let dir = root.path().join("iter-no-dispatch");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            resolve_agent_description(&dir, "crash", "with_skill", None),
            "crash:with_skill"
        );
    }

    #[test]
    fn falls_back_when_task_missing_from_dispatch() {
        let root = TempDir::new().unwrap();
        let dir = root.path().join("iter-partial");
        write_dispatch(
            &dir,
            json!([{"eval_id": "other", "condition": "with_skill", "agent_description": "other:with_skill:i1-x"}]),
        );
        assert_eq!(
            resolve_agent_description(&dir, "crash", "with_skill", None),
            "crash:with_skill"
        );
    }

    #[test]
    fn falls_back_when_dispatch_malformed() {
        let root = TempDir::new().unwrap();
        let dir = root.path().join("iter-malformed");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("dispatch.json"), "{ not valid json").unwrap();
        assert_eq!(
            resolve_agent_description(&dir, "crash", "with_skill", None),
            "crash:with_skill"
        );
    }

    // --- fillTranscripts ---

    #[test]
    fn fills_a_codex_run_record_from_outputs_events() {
        let root = TempDir::new().unwrap();
        let iteration_dir: PathBuf = root.path().join("iter-codex-fill");
        let cond_dir = iteration_dir.join("eval-crash").join("with_skill");
        let outputs_dir = cond_dir.join("outputs");
        fs::create_dir_all(&outputs_dir).unwrap();
        let run_path = cond_dir.join("run.json");
        write_run_record(&run_path, json!([]));
        fs::write(
            iteration_dir.join("conditions.json"),
            json!({
                "mode": "new-skill",
                "conditions": [{"name": "with_skill", "skill_path": "/skill/SKILL.md"}],
                "timestamp": "2026-06-07T00:00:00.000Z",
                "harness": "codex"
            })
            .to_string(),
        )
        .unwrap();
        write_dispatch(
            &iteration_dir,
            json!([{"eval_id": "crash", "condition": "with_skill", "outputs_dir": outputs_dir.to_string_lossy()}]),
        );
        fs::write(
            outputs_dir.join("codex-events.jsonl"),
            jsonl(&[
                json!({"type": "item.completed", "item": {"id": "item_1", "type": "command_execution", "command": "bun test", "output": "ok"}}),
            ]),
        )
        .unwrap();

        let result = fill_transcripts(&iteration_dir, Harness::Codex, None, false).unwrap();
        assert_eq!(result.filled, 1);
        assert_eq!(result.missing, 0);

        let updated: RunRecord =
            serde_json::from_str(&fs::read_to_string(&run_path).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(&updated.tool_invocations).unwrap(),
            json!([{"name": "command_execution", "ordinal": 0, "args": {"command": "bun test"}, "result": "ok"}])
        );
    }

    #[test]
    fn fills_codex_run_records_in_nested_run_dirs() {
        let root = TempDir::new().unwrap();
        let iteration_dir: PathBuf = root.path().join("iter-codex-multi");
        let cond_dir = iteration_dir.join("eval-crash").join("with_skill");
        fs::create_dir_all(&iteration_dir).unwrap();
        fs::write(
            iteration_dir.join("conditions.json"),
            json!({
                "mode": "new-skill",
                "conditions": [{"name": "with_skill", "skill_path": "/skill/SKILL.md"}],
                "timestamp": "2026-06-07T00:00:00.000Z",
                "harness": "codex"
            })
            .to_string(),
        )
        .unwrap();
        for (k, command) in [(1, "bun test"), (2, "bun lint")] {
            let run_dir = cond_dir.join(format!("run-{k}"));
            let outputs_dir = run_dir.join("outputs");
            fs::create_dir_all(&outputs_dir).unwrap();
            write_run_record(&run_dir.join("run.json"), json!([]));
            fs::write(
                outputs_dir.join("codex-events.jsonl"),
                jsonl(&[
                    json!({"type": "item.completed", "item": {"id": "item_1", "type": "command_execution", "command": command, "output": "ok"}}),
                ]),
            )
            .unwrap();
        }

        let result = fill_transcripts(&iteration_dir, Harness::Codex, None, false).unwrap();
        assert_eq!(result.filled, 2);
        assert_eq!(result.missing, 0);

        for (k, command) in [(1, "bun test"), (2, "bun lint")] {
            let updated: RunRecord = serde_json::from_str(
                &fs::read_to_string(cond_dir.join(format!("run-{k}")).join("run.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                serde_json::to_value(&updated.tool_invocations).unwrap(),
                json!([{"name": "command_execution", "ordinal": 0, "args": {"command": command}, "result": "ok"}]),
                "wrong invocations for run-{k}"
            );
        }
    }
}
