//! Stage 1 — `record-runs`.
//!
//! Assembles a schema-valid `run.json` (and
//! backfills `timing.json`) for every task in the iteration's `dispatch.json`,
//! from sources already on disk: carry-over fields from the dispatch task, the
//! `final_message` (from `<outputs_dir>/final-message.md`, falling back to the
//! transcript's last assistant text), and `tool_invocations`/tokens/duration from
//! the persisted transcript (Claude Code subagent JSONL or Codex
//! `codex-events.jsonl`).
//!
//! Existing records always win: an agent/operator-written `run.json` is skipped
//! without `overwrite`, and `timing.json` is backfill-only — completion-event
//! numbers captured at dispatch time are never replaced by transcript-derived
//! ones (which include cache accounting and are not comparable 1:1).

use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::adapters::{
    TranscriptSummary, find_by_description, parse_codex_events_full, parse_transcript_full,
};
use crate::core::{Harness, RunRecord, TimingRecord, TimingSource};
use crate::pipeline::error::PipelineError;
use crate::pipeline::io::write_json;
use crate::validation::{SchemaName, validate_against_schema};

/// The `dispatch.json` envelope record-runs reads.
#[derive(Debug, Deserialize)]
struct DispatchFile {
    tasks: Option<Vec<DispatchTask>>,
}

/// The subset of a dispatch task record-runs consumes. `dispatch.json` carries
/// more fields (e.g. `staged_skill_slug`, `dispatch_prompt_path`); serde ignores
/// the extras.
#[derive(Debug, Deserialize)]
struct DispatchTask {
    eval_id: String,
    condition: String,
    #[serde(default)]
    run_index: Option<u32>,
    skill_path: Option<String>,
    user_prompt: String,
    fixtures: Vec<String>,
    outputs_dir: String,
    run_record_path: String,
    timing_path: String,
    agent_description: String,
}

/// Tally of what record-runs did across the dispatch's tasks.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecordRunsResult {
    pub recorded: usize,
    pub skipped_existing: usize,
    pub skipped_no_final_message: usize,
    pub missing_transcript: usize,
}

impl RecordRunsResult {
    /// A loud, actionable warning when runs were recorded from `final-message.md`
    /// but their transcripts didn't link — leaving `tool_invocations`/tokens/
    /// duration empty so `transcript_check` assertions silently grade
    /// unverifiable. `None` when every run matched its transcript. The hint is
    /// tailored to how the harness correlates transcripts (description match vs.
    /// the Codex events file).
    pub fn transcript_warning(&self, harness: Harness) -> Option<String> {
        if self.missing_transcript == 0 {
            return None;
        }
        let n = self.missing_transcript;
        let plural = if n == 1 { "" } else { "s" };
        let all = self.recorded > 0 && self.missing_transcript >= self.recorded;
        let lead = if all {
            format!("⚠ {n} run{plural} recorded but NONE matched a transcript")
        } else {
            format!("⚠ {n} run{plural} missing a transcript")
        };
        let cause = match harness {
            Harness::Codex => "expected `outputs/codex-events.jsonl` was not found".to_string(),
            Harness::ClaudeCode | Harness::OpenCode => {
                "did you pass each task's `agent_description` verbatim as the subagent \
                 description? If so, confirm `--subagents-dir` points at the parent session's \
                 subagents dir"
                    .to_string()
            }
        };
        Some(format!(
            "{lead} — {cause}; tool_invocations/tokens/duration are empty, so transcript_check \
             assertions will grade unverifiable."
        ))
    }
}

/// Assemble `run.json` + `timing.json` for every task in
/// `<iteration_dir>/dispatch.json`. See the module docs for the field sources and
/// the existing-record precedence rules.
pub fn record_runs(
    iteration_dir: &Path,
    harness: Harness,
    subagents_dir: Option<&Path>,
    overwrite: bool,
) -> Result<RecordRunsResult, PipelineError> {
    let dispatch_path = iteration_dir.join("dispatch.json");
    if !dispatch_path.exists() {
        return Err(PipelineError::Message(format!(
            "{} not found — record-runs assembles records from dispatch.json and only \
             supports runner-built iterations. For hand-authored runs, write run.json + \
             timing.json manually (see schema/run-record.schema.json).",
            dispatch_path.display()
        )));
    }
    let dispatch: DispatchFile = serde_json::from_str(&fs::read_to_string(&dispatch_path)?)?;
    let tasks = dispatch.tasks.unwrap_or_default();

    let mut result = RecordRunsResult::default();
    for task in &tasks {
        let summary = transcript_summary_for_task(harness, subagents_dir, task);
        if summary.is_none() {
            result.missing_transcript += 1;
        }

        let run_record_path = Path::new(&task.run_record_path);
        if run_record_path.exists() && !overwrite {
            // An agent/operator already wrote this run.json — leave it untouched.
            result.skipped_existing += 1;
        } else {
            let final_message_path = Path::new(&task.outputs_dir).join("final-message.md");
            let final_message = if final_message_path.exists() {
                Some(fs::read_to_string(&final_message_path)?.trim().to_string())
            } else {
                summary.as_ref().and_then(|s| s.final_text.clone())
            };
            let Some(final_message) = final_message else {
                // No final-message.md and no transcript text — don't write a blank record.
                result.skipped_no_final_message += 1;
                continue;
            };

            let record = RunRecord {
                eval_id: task.eval_id.clone(),
                condition: task.condition.clone(),
                skill_path: task.skill_path.clone(),
                prompt: task.user_prompt.clone(),
                files: task.fixtures.clone(),
                final_message,
                tool_invocations: summary
                    .as_ref()
                    .map(|s| s.tool_invocations.clone())
                    .unwrap_or_default(),
                // Timing lives in timing.json; run.json never carries it.
                total_tokens: None,
                duration_ms: None,
                run_index: task.run_index,
            };
            validate_against_schema::<RunRecord>(
                SchemaName::RunRecord,
                &serde_json::to_value(&record)?,
                &task.run_record_path,
            )?;
            write_json(run_record_path, &record)?;
            result.recorded += 1;
        }

        // timing.json — backfill only; completion-event numbers always win.
        let timing_path = Path::new(&task.timing_path);
        if let Some(summary) = &summary
            && (!timing_path.exists() || overwrite)
        {
            let timing = TimingRecord {
                total_tokens: Some(summary.total_tokens),
                duration_ms: Some(summary.duration_ms),
                source: Some(TimingSource::Transcript),
            };
            write_json(timing_path, &timing)?;
        }
    }

    Ok(result)
}

/// Resolve a task's transcript summary: a Codex `codex-events.jsonl` under the
/// task's outputs dir, or the Claude Code subagent transcript matched by the
/// task's `agent_description`. Returns `None` when no transcript is found.
fn transcript_summary_for_task(
    harness: Harness,
    subagents_dir: Option<&Path>,
    task: &DispatchTask,
) -> Option<TranscriptSummary> {
    if harness == Harness::Codex {
        let events_path = Path::new(&task.outputs_dir).join("codex-events.jsonl");
        if !events_path.exists() {
            return None;
        }
        return parse_codex_events_full(&events_path).ok();
    }

    let subagent = find_by_description(
        subagents_dir.unwrap_or_else(|| Path::new("")),
        &task.agent_description,
    )?;
    parse_transcript_full(&subagent.jsonl_path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Token math for `transcript_lines`: msg_1 (100+20+30+50) + msg_2
    /// (200+40+0+60) = 500.
    const TRANSCRIPT_TOKENS: i64 = 500;
    /// 10:00:00.000 → 10:01:00.000.
    const TRANSCRIPT_DURATION_MS: i64 = 60_000;

    fn jsonl(lines: &[Value]) -> String {
        let body = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        format!("{body}\n")
    }

    /// A minimal transcript with usage, timestamps, one tool call, and final text.
    fn transcript_lines(final_text: &str) -> Vec<Value> {
        vec![
            json!({"type": "user", "timestamp": "2026-06-04T10:00:00.000Z", "message": {"role": "user", "content": "go"}}),
            json!({"type": "assistant", "timestamp": "2026-06-04T10:00:10.000Z", "message": {
                "id": "msg_1", "role": "assistant",
                "usage": {"input_tokens": 100, "output_tokens": 20, "cache_creation_input_tokens": 30, "cache_read_input_tokens": 50},
                "content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}]
            }}),
            json!({"type": "user", "timestamp": "2026-06-04T10:00:12.000Z", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}}),
            json!({"type": "assistant", "timestamp": "2026-06-04T10:01:00.000Z", "message": {
                "id": "msg_2", "role": "assistant",
                "usage": {"input_tokens": 200, "output_tokens": 40, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 60},
                "content": [{"type": "text", "text": final_text}]
            }}),
        ]
    }

    fn write_subagent(subagents_dir: &Path, name: &str, description: &str, lines: &[Value]) {
        fs::write(
            subagents_dir.join(format!("{name}.meta.json")),
            json!({"agentType": "general-purpose", "description": description}).to_string(),
        )
        .unwrap();
        fs::write(subagents_dir.join(format!("{name}.jsonl")), jsonl(lines)).unwrap();
    }

    fn write_codex_events(outputs_dir: &Path, final_text: &str) {
        let lines = vec![
            json!({"type": "thread.started", "timestamp": "2026-06-04T10:00:00.000Z"}),
            json!({"type": "item.completed", "timestamp": "2026-06-04T10:00:10.000Z", "item": {"id": "item_1", "type": "command_execution", "command": "bun test", "output": "ok"}}),
            json!({"type": "item.completed", "timestamp": "2026-06-04T10:00:20.000Z", "item": {"id": "item_2", "type": "agent_message", "text": final_text}}),
            json!({"type": "turn.completed", "timestamp": "2026-06-04T10:00:30.000Z", "usage": {"input_tokens": 100, "cached_input_tokens": 80, "output_tokens": 20, "reasoning_output_tokens": 5}}),
        ];
        fs::write(outputs_dir.join("codex-events.jsonl"), jsonl(&lines)).unwrap();
    }

    struct FixtureTask {
        eval_id: &'static str,
        condition: &'static str,
        /// Written to `outputs/final-message.md` when `Some`.
        final_message: Option<&'static str>,
    }

    /// Paths the tests reach into after building the iteration.
    struct TaskPaths {
        outputs_dir: PathBuf,
        run_record_path: PathBuf,
        timing_path: PathBuf,
    }

    /// Build an iteration dir + `dispatch.json` shaped like `run.ts` serializes it.
    fn write_iteration(iteration_dir: &Path, tasks: &[FixtureTask]) -> Vec<TaskPaths> {
        let mut serialized = Vec::new();
        let mut paths = Vec::new();
        for t in tasks {
            let cond_dir = iteration_dir
                .join(format!("eval-{}", t.eval_id))
                .join(t.condition);
            let outputs_dir = cond_dir.join("outputs");
            fs::create_dir_all(&outputs_dir).unwrap();
            if let Some(msg) = t.final_message {
                fs::write(outputs_dir.join("final-message.md"), msg).unwrap();
            }
            let run_record_path = cond_dir.join("run.json");
            let timing_path = cond_dir.join("timing.json");
            let without = t.condition == "without_skill";
            serialized.push(json!({
                "eval_id": t.eval_id,
                "condition": t.condition,
                "skill_path": if without { Value::Null } else { json!("/staged/skill/SKILL.md") },
                "staged_skill_slug": if without { Value::Null } else { json!("test-slug") },
                "user_prompt": format!("Do the {} task", t.eval_id),
                "fixtures": [cond_dir.join("inputs").join("fixture.txt").to_string_lossy()],
                "outputs_dir": outputs_dir.to_string_lossy(),
                "run_record_path": run_record_path.to_string_lossy(),
                "timing_path": timing_path.to_string_lossy(),
                "agent_description": format!("{}:{}:i1-nonce1", t.eval_id, t.condition),
                "dispatch_prompt_path": cond_dir.join("dispatch-prompt.txt").to_string_lossy(),
            }));
            paths.push(TaskPaths {
                outputs_dir,
                run_record_path,
                timing_path,
            });
        }
        fs::write(
            iteration_dir.join("dispatch.json"),
            serde_json::to_string_pretty(&json!({"run_nonce": "nonce1", "tasks": serialized}))
                .unwrap(),
        )
        .unwrap();
        paths
    }

    fn read_run(iteration_dir: &Path, eval_id: &str, condition: &str) -> RunRecord {
        let raw = fs::read_to_string(
            iteration_dir
                .join(format!("eval-{eval_id}"))
                .join(condition)
                .join("run.json"),
        )
        .unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    fn read_timing_value(iteration_dir: &Path, eval_id: &str, condition: &str) -> Value {
        let raw = fs::read_to_string(
            iteration_dir
                .join(format!("eval-{eval_id}"))
                .join(condition)
                .join("timing.json"),
        )
        .unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    fn timing_exists(iteration_dir: &Path, eval_id: &str, condition: &str) -> bool {
        iteration_dir
            .join(format!("eval-{eval_id}"))
            .join(condition)
            .join("timing.json")
            .exists()
    }

    fn run_exists(iteration_dir: &Path, eval_id: &str, condition: &str) -> bool {
        iteration_dir
            .join(format!("eval-{eval_id}"))
            .join(condition)
            .join("run.json")
            .exists()
    }

    /// `(iteration_dir, subagents_dir)` under a fresh temp root.
    fn dirs(root: &TempDir) -> (PathBuf, PathBuf) {
        let iteration_dir = root.path().join("iter");
        let subagents_dir = root.path().join("sub");
        fs::create_dir_all(&iteration_dir).unwrap();
        fs::create_dir_all(&subagents_dir).unwrap();
        (iteration_dir, subagents_dir)
    }

    #[test]
    fn assembles_run_and_timing_for_every_task_from_disk() {
        let root = TempDir::new().unwrap();
        let (iter, sub) = dirs(&root);
        write_iteration(
            &iter,
            &[
                FixtureTask {
                    eval_id: "crash",
                    condition: "with_skill",
                    final_message: Some("Fixed it."),
                },
                FixtureTask {
                    eval_id: "crash",
                    condition: "without_skill",
                    final_message: Some("Done, I think."),
                },
            ],
        );
        write_subagent(
            &sub,
            "agent-a",
            "crash:with_skill:i1-nonce1",
            &transcript_lines("unused"),
        );
        write_subagent(
            &sub,
            "agent-b",
            "crash:without_skill:i1-nonce1",
            &transcript_lines("unused"),
        );

        let result = record_runs(&iter, Harness::ClaudeCode, Some(&sub), false).unwrap();
        assert_eq!(result.recorded, 2);
        assert_eq!(result.missing_transcript, 0);

        let run = read_run(&iter, "crash", "with_skill");
        assert_eq!(run.eval_id, "crash");
        assert_eq!(run.condition, "with_skill");
        assert_eq!(run.skill_path.as_deref(), Some("/staged/skill/SKILL.md"));
        assert_eq!(run.prompt, "Do the crash task");
        assert_eq!(run.files.len(), 1);
        assert_eq!(run.final_message, "Fixed it.");
        assert_eq!(run.tool_invocations.len(), 1);
        assert_eq!(run.tool_invocations[0].name, "Bash");
        assert_eq!(run.tool_invocations[0].ordinal, 0);

        assert!(
            read_run(&iter, "crash", "without_skill")
                .skill_path
                .is_none()
        );

        let timing = read_timing_value(&iter, "crash", "with_skill");
        assert_eq!(timing["total_tokens"], json!(TRANSCRIPT_TOKENS));
        assert_eq!(timing["duration_ms"], json!(TRANSCRIPT_DURATION_MS));
        assert_eq!(timing["source"], json!("transcript"));
    }

    #[test]
    fn carries_run_index_from_dispatch_task_into_each_run_record() {
        let root = TempDir::new().unwrap();
        let (iter, _sub) = dirs(&root);
        let cond_dir = iter.join("eval-crash").join("with_skill");
        let mut serialized = Vec::new();
        for k in [1u32, 2] {
            let run_dir = cond_dir.join(format!("run-{k}"));
            let outputs_dir = run_dir.join("outputs");
            fs::create_dir_all(&outputs_dir).unwrap();
            fs::write(
                outputs_dir.join("final-message.md"),
                format!("Fixed it in run {k}."),
            )
            .unwrap();
            write_codex_events(&outputs_dir, "unused");
            serialized.push(json!({
                "eval_id": "crash",
                "condition": "with_skill",
                "run_index": k,
                "skill_path": "/staged/skill/SKILL.md",
                "user_prompt": "Do the crash task",
                "fixtures": [],
                "outputs_dir": outputs_dir.to_string_lossy(),
                "run_record_path": run_dir.join("run.json").to_string_lossy(),
                "timing_path": run_dir.join("timing.json").to_string_lossy(),
                "agent_description": format!("crash:with_skill:r{k}:i1-nonce1"),
            }));
        }
        fs::write(
            iter.join("dispatch.json"),
            serde_json::to_string_pretty(&json!({"run_nonce": "nonce1", "tasks": serialized}))
                .unwrap(),
        )
        .unwrap();

        let result = record_runs(&iter, Harness::Codex, None, false).unwrap();
        assert_eq!(result.recorded, 2);

        for k in [1u32, 2] {
            let raw =
                fs::read_to_string(cond_dir.join(format!("run-{k}")).join("run.json")).unwrap();
            let run: RunRecord = serde_json::from_str(&raw).unwrap();
            assert_eq!(run.run_index, Some(k), "wrong run_index for run-{k}");
            assert_eq!(run.final_message, format!("Fixed it in run {k}."));
        }
    }

    #[test]
    fn assembles_codex_records_from_each_tasks_events() {
        let root = TempDir::new().unwrap();
        let (iter, _sub) = dirs(&root);
        let paths = write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: Some("Fixed it."),
            }],
        );
        write_codex_events(&paths[0].outputs_dir, "Codex final.");

        let result = record_runs(&iter, Harness::Codex, None, false).unwrap();
        assert_eq!(result.recorded, 1);
        assert_eq!(result.missing_transcript, 0);

        let run = read_run(&iter, "crash", "with_skill");
        assert_eq!(run.final_message, "Fixed it.");
        assert_eq!(
            serde_json::to_value(&run.tool_invocations).unwrap(),
            json!([{"name": "command_execution", "ordinal": 0, "args": {"command": "bun test"}, "result": "ok"}])
        );

        let timing = read_timing_value(&iter, "crash", "with_skill");
        assert_eq!(
            timing,
            json!({"total_tokens": 125, "duration_ms": 30_000, "source": "transcript"})
        );
    }

    #[test]
    fn falls_back_to_codex_final_agent_message_when_final_message_md_missing() {
        let root = TempDir::new().unwrap();
        let (iter, _sub) = dirs(&root);
        let paths = write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: None,
            }],
        );
        write_codex_events(&paths[0].outputs_dir, "Closing summary from Codex.");

        let result = record_runs(&iter, Harness::Codex, None, false).unwrap();
        assert_eq!(result.recorded, 1);
        assert_eq!(
            read_run(&iter, "crash", "with_skill").final_message,
            "Closing summary from Codex."
        );
    }

    #[test]
    fn skips_existing_run_without_overwrite_then_replaces_with_it() {
        let root = TempDir::new().unwrap();
        let (iter, sub) = dirs(&root);
        let paths = write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: Some("New."),
            }],
        );
        write_subagent(
            &sub,
            "agent-a",
            "crash:with_skill:i1-nonce1",
            &transcript_lines("unused"),
        );
        let hand_written = json!({
            "eval_id": "crash", "condition": "with_skill",
            "skill_path": "/staged/skill/SKILL.md", "prompt": "Do the crash task",
            "files": [], "final_message": "Agent-authored.", "tool_invocations": []
        });
        fs::write(&paths[0].run_record_path, hand_written.to_string()).unwrap();

        let skipped = record_runs(&iter, Harness::ClaudeCode, Some(&sub), false).unwrap();
        assert_eq!(skipped.recorded, 0);
        assert_eq!(skipped.skipped_existing, 1);
        assert_eq!(
            read_run(&iter, "crash", "with_skill").final_message,
            "Agent-authored."
        );

        let replaced = record_runs(&iter, Harness::ClaudeCode, Some(&sub), true).unwrap();
        assert_eq!(replaced.recorded, 1);
        assert_eq!(read_run(&iter, "crash", "with_skill").final_message, "New.");
    }

    #[test]
    fn backfills_timing_only_when_absent() {
        let root = TempDir::new().unwrap();
        let (iter, sub) = dirs(&root);
        let paths = write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: Some("Done."),
            }],
        );
        write_subagent(
            &sub,
            "agent-a",
            "crash:with_skill:i1-nonce1",
            &transcript_lines("unused"),
        );
        fs::write(
            &paths[0].timing_path,
            json!({"total_tokens": 12345, "duration_ms": 9000}).to_string(),
        )
        .unwrap();

        record_runs(&iter, Harness::ClaudeCode, Some(&sub), false).unwrap();

        // Agent-captured completion-event timing wins; not overwritten.
        let timing = read_timing_value(&iter, "crash", "with_skill");
        assert_eq!(timing["total_tokens"], json!(12345));
        assert_eq!(timing["duration_ms"], json!(9000));
        assert!(timing.get("source").is_none());
    }

    #[test]
    fn falls_back_to_transcript_final_assistant_text_when_final_message_md_missing() {
        let root = TempDir::new().unwrap();
        let (iter, sub) = dirs(&root);
        write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: None,
            }],
        );
        write_subagent(
            &sub,
            "agent-a",
            "crash:with_skill:i1-nonce1",
            &transcript_lines("Closing summary from transcript."),
        );

        let result = record_runs(&iter, Harness::ClaudeCode, Some(&sub), false).unwrap();
        assert_eq!(result.recorded, 1);
        assert_eq!(
            read_run(&iter, "crash", "with_skill").final_message,
            "Closing summary from transcript."
        );
    }

    #[test]
    fn skips_the_slot_entirely_when_no_final_message_source_exists() {
        let root = TempDir::new().unwrap();
        let (iter, sub) = dirs(&root);
        write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: None,
            }],
        );
        // No final-message.md, no transcript.

        let result = record_runs(&iter, Harness::ClaudeCode, Some(&sub), false).unwrap();
        assert_eq!(result.recorded, 0);
        assert_eq!(result.skipped_no_final_message, 1);
        assert!(!run_exists(&iter, "crash", "with_skill"));
        assert!(!timing_exists(&iter, "crash", "with_skill"));
    }

    #[test]
    fn writes_empty_invocations_and_no_timing_when_transcript_missing() {
        let root = TempDir::new().unwrap();
        let (iter, sub) = dirs(&root);
        write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: Some("Done."),
            }],
        );
        // final-message.md exists but no subagent transcript matches.

        let result = record_runs(&iter, Harness::ClaudeCode, Some(&sub), false).unwrap();
        assert_eq!(result.recorded, 1);
        assert_eq!(result.missing_transcript, 1);

        let run = read_run(&iter, "crash", "with_skill");
        assert_eq!(run.final_message, "Done.");
        assert!(run.tool_invocations.is_empty());
        assert!(!timing_exists(&iter, "crash", "with_skill"));
    }

    #[test]
    fn errors_when_dispatch_json_is_absent() {
        let root = TempDir::new().unwrap();
        let (iter, sub) = dirs(&root);
        // Hand-authored/operator runs have no dispatch.json — the manual path owns them.
        let err = record_runs(&iter, Harness::ClaudeCode, Some(&sub), false).unwrap_err();
        assert!(
            err.to_string().contains("dispatch.json"),
            "error was: {err}"
        );
    }

    #[test]
    fn no_transcript_warning_when_all_transcripts_matched() {
        let result = RecordRunsResult {
            recorded: 4,
            missing_transcript: 0,
            ..Default::default()
        };
        assert!(result.transcript_warning(Harness::ClaudeCode).is_none());
    }

    #[test]
    fn claude_code_warning_names_agent_description_when_all_runs_miss() {
        let result = RecordRunsResult {
            recorded: 8,
            missing_transcript: 8,
            ..Default::default()
        };
        let warning = result.transcript_warning(Harness::ClaudeCode).unwrap();
        assert!(warning.contains("8"), "names the count: {warning}");
        assert!(
            warning.contains("agent_description"),
            "points at the load-bearing key: {warning}"
        );
        assert!(
            warning.to_lowercase().contains("verbatim"),
            "says pass it verbatim: {warning}"
        );
        assert!(
            warning.contains("--subagents-dir"),
            "offers the other likely cause: {warning}"
        );
    }

    #[test]
    fn claude_code_warning_fires_on_partial_miss() {
        let result = RecordRunsResult {
            recorded: 4,
            missing_transcript: 1,
            ..Default::default()
        };
        let warning = result.transcript_warning(Harness::ClaudeCode).unwrap();
        assert!(warning.contains('1'), "names the count: {warning}");
    }

    #[test]
    fn codex_warning_points_at_events_file() {
        let result = RecordRunsResult {
            recorded: 2,
            missing_transcript: 2,
            ..Default::default()
        };
        let warning = result.transcript_warning(Harness::Codex).unwrap();
        assert!(
            warning.contains("codex-events.jsonl"),
            "names the Codex source: {warning}"
        );
        assert!(
            !warning.contains("agent_description"),
            "Codex doesn't use agent_description: {warning}"
        );
    }
}
