//! Stage 1 — `record-runs`.
//!
//! Assembles a schema-valid `run.json` (and
//! backfills `timing.json`) for every task in the iteration's `dispatch.json`,
//! from sources already on disk: carry-over fields from the dispatch task, the
//! `final_message` (from `<outputs_dir>/final-message.md`, falling back to the
//! transcript's last assistant text), and `tool_invocations`/tokens/duration from
//! each task's events file (`outputs/<harness>-events.jsonl` — Claude Code's
//! `claude -p` stream-json or Codex's `codex-events.jsonl`).
//!
//! Existing records always win: an agent/operator-written `run.json` is skipped
//! without `overwrite`, and `timing.json` is backfill-only — completion-event
//! numbers captured at dispatch time are never replaced by transcript-derived
//! ones (which include cache accounting and are not comparable 1:1).

use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::adapters::{TranscriptSummary, adapter_for};
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
/// more fields (e.g. `staged_skill_slug`); serde ignores the extras.
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
    #[serde(default)]
    dispatch_prompt_path: String,
}

/// Tally of what record-runs did across the dispatch's tasks.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecordRunsResult {
    pub recorded: usize,
    pub skipped_existing: usize,
    pub skipped_no_final_message: usize,
    pub missing_transcript: usize,
    pub skipped_prompt_unread: usize,
}

impl RecordRunsResult {
    /// A loud, actionable warning when runs were recorded from `final-message.md`
    /// but their transcripts didn't link — leaving `tool_invocations`/tokens/
    /// duration empty so `transcript_check` assertions silently grade
    /// unverifiable. `None` when every run matched its transcript. The hint names
    /// the per-task events file the harness CLI was expected to write.
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
        let file = adapter_for(harness)
            .cli_events_filename()
            .unwrap_or("the events file");
        let cause = format!("expected `outputs/{file}` was not found");
        Some(format!(
            "{lead} — {cause}; tool_invocations/tokens/duration are empty, so transcript_check \
             assertions will grade unverifiable."
        ))
    }

    /// A loud, actionable warning when one or more dispatches were excluded
    /// because their transcript shows a failed read of the dispatch prompt — the
    /// agent never received its instructions, so the result is a no-op, not data.
    /// `None` when none were flagged.
    pub fn prompt_unread_warning(&self) -> Option<String> {
        if self.skipped_prompt_unread == 0 {
            return None;
        }
        let n = self.skipped_prompt_unread;
        let plural = if n == 1 { "" } else { "es" };
        Some(format!(
            "⚠ {n} dispatch{plural} skipped — the transcript shows a failed read of the dispatch \
             prompt (the agent never received its instructions). These are NOT recorded, so they \
             cannot be graded as data. Check the env/sandbox can reach each task's \
             `dispatch_prompt_path`, then re-dispatch."
        ))
    }
}

/// Assemble `run.json` + `timing.json` for every task in
/// `<iteration_dir>/dispatch.json`. See the module docs for the field sources and
/// the existing-record precedence rules.
pub fn record_runs(
    iteration_dir: &Path,
    harness: Harness,
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
        let summary = transcript_summary_for_task(harness, task);
        if summary.is_none() {
            result.missing_transcript += 1;
        }

        let run_record_path = Path::new(&task.run_record_path);
        if run_record_path.exists() && !overwrite {
            // An agent/operator already wrote this run.json — leave it untouched.
            result.skipped_existing += 1;
        } else if let Some(summary) = &summary
            && prompt_read_failed(
                summary,
                &task.dispatch_prompt_path,
                &prompt_sentinel(&task.dispatch_prompt_path),
            )
        {
            // The transcript shows the agent tried to read its prompt and the
            // read returned an error, not the prompt — it never received its
            // instructions. Skip both run.json and timing so the no-op can't be
            // graded as data.
            result.skipped_prompt_unread += 1;
            continue;
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

/// Positive evidence that the agent tried to read its dispatch prompt and
/// failed: the transcript has a tool call referencing `prompt_path`, yet no such
/// call returned the prompt's content (its distinctive first-line `sentinel`).
///
/// A run that never references the prompt path is NOT flagged — absence is not
/// proof of failure (the agent can receive the prompt another way),
/// and requiring positive evidence keeps the check free of false positives.
/// Returns `false` when `sentinel` is empty (the prompt file was missing or
/// unreadable, so the read cannot be judged).
fn prompt_read_failed(summary: &TranscriptSummary, prompt_path: &str, sentinel: &str) -> bool {
    if sentinel.is_empty() {
        return false;
    }
    let mut referenced = false;
    let mut delivered = false;
    for inv in &summary.tool_invocations {
        let mentions_prompt = inv
            .args
            .as_ref()
            .is_some_and(|a| a.to_string().contains(prompt_path));
        if !mentions_prompt {
            continue;
        }
        referenced = true;
        if inv
            .result
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .is_some_and(|r| r.contains(sentinel))
        {
            delivered = true;
        }
    }
    referenced && !delivered
}

/// The dispatch prompt's distinctive first non-empty line, used as the sentinel
/// for [`prompt_read_failed`]. Empty when the prompt file is missing/unreadable.
fn prompt_sentinel(prompt_path: &str) -> String {
    if prompt_path.is_empty() {
        return String::new();
    }
    fs::read_to_string(prompt_path)
        .ok()
        .and_then(|p| {
            p.lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
        })
        .unwrap_or_default()
}

/// Resolve a task's transcript summary: read the events file the harness CLI
/// wrote under the task's outputs dir (e.g. Codex's `codex-events.jsonl`, Claude
/// Code's `claude-events.jsonl`). Returns `None` when no transcript is found.
fn transcript_summary_for_task(harness: Harness, task: &DispatchTask) -> Option<TranscriptSummary> {
    let events_path =
        Path::new(&task.outputs_dir).join(adapter_for(harness).cli_events_filename()?);
    if !events_path.exists() {
        return None;
    }
    adapter_for(harness)
        .parse_cli_events_full(&events_path)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn jsonl(lines: &[Value]) -> String {
        let body = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        format!("{body}\n")
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

    /// A `claude -p --output-format stream-json` events fixture: a `system/init`
    /// line, one tool call, and a terminal `result` event carrying the final
    /// text + duration + usage (there are no per-line timestamps). Tokens sum to
    /// 125 (100 + 20 + 0 + 5).
    fn write_claude_events(outputs_dir: &Path, final_text: &str) {
        let lines = vec![
            json!({"type": "system", "subtype": "init", "cwd": "/env"}),
            json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "bun test"}}]}}),
            json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}}),
            json!({"type": "result", "subtype": "success", "is_error": false, "result": final_text, "duration_ms": 30_000, "usage": {"input_tokens": 100, "output_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 5}}),
        ];
        fs::write(outputs_dir.join("claude-events.jsonl"), jsonl(&lines)).unwrap();
    }

    /// A `claude -p` events fixture where the agent reads its dispatch prompt:
    /// a `Read` tool call whose `input.file_path` is `prompt_path`, a
    /// `tool_result` carrying `read_result` (the file content on success, an
    /// error string on a denied/out-of-cwd read), and a terminal `result` event.
    fn write_claude_events_prompt_read(
        outputs_dir: &Path,
        prompt_path: &str,
        read_result: &str,
        final_text: &str,
    ) {
        let lines = vec![
            json!({"type": "system", "subtype": "init", "cwd": "/env"}),
            json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"file_path": prompt_path}}]}}),
            json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": read_result}]}}),
            json!({"type": "result", "subtype": "success", "is_error": false, "result": final_text, "duration_ms": 30_000, "usage": {"input_tokens": 100, "output_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 5}}),
        ];
        fs::write(outputs_dir.join("claude-events.jsonl"), jsonl(&lines)).unwrap();
    }

    const PROMPT_SENTINEL: &str =
        "You are executing a single test case for a skill evaluation framework.";

    #[test]
    fn flags_dispatch_whose_prompt_read_failed() {
        // A dispatch that couldn't read its prompt still exits 0 and emits a
        // final message — but the run is a silent no-op, not data (issue #109).
        let tmp = TempDir::new().unwrap();
        let iter = tmp.path();
        let paths = write_iteration(
            iter,
            &[FixtureTask {
                eval_id: "e1",
                condition: "with_skill",
                final_message: Some("I could not read the prompt file."),
            }],
        );
        let prompt_path = iter
            .join("eval-e1")
            .join("with_skill")
            .join("dispatch-prompt.txt");
        fs::write(
            &prompt_path,
            format!("{PROMPT_SENTINEL}\n\nUser request:\ndo it"),
        )
        .unwrap();
        // The transcript shows a Read of the prompt path that ERRORED — the
        // result is a denial, not the prompt content.
        write_claude_events_prompt_read(
            &paths[0].outputs_dir,
            &prompt_path.to_string_lossy(),
            "<tool_use_error>File is outside the allowed working directory.</tool_use_error>",
            "I could not read the prompt file.",
        );

        let result = record_runs(iter, Harness::ClaudeCode, false).unwrap();

        assert_eq!(result.skipped_prompt_unread, 1);
        assert_eq!(result.recorded, 0);
        assert!(!paths[0].run_record_path.exists());
    }

    #[test]
    fn records_dispatch_when_prompt_read_succeeded() {
        // The same shape, but the Read returned the prompt content (Read echoes
        // it with a line-number prefix) — a legitimate run, recorded as data.
        let tmp = TempDir::new().unwrap();
        let iter = tmp.path();
        let paths = write_iteration(
            iter,
            &[FixtureTask {
                eval_id: "e1",
                condition: "with_skill",
                final_message: Some("Done."),
            }],
        );
        let prompt_path = iter
            .join("eval-e1")
            .join("with_skill")
            .join("dispatch-prompt.txt");
        fs::write(
            &prompt_path,
            format!("{PROMPT_SENTINEL}\n\nUser request:\ndo it"),
        )
        .unwrap();
        write_claude_events_prompt_read(
            &paths[0].outputs_dir,
            &prompt_path.to_string_lossy(),
            &format!("     1→{PROMPT_SENTINEL}\n     2→\n     3→User request:"),
            "Done.",
        );

        let result = record_runs(iter, Harness::ClaudeCode, false).unwrap();

        assert_eq!(result.recorded, 1);
        assert_eq!(result.skipped_prompt_unread, 0);
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

    /// The iteration dir under a fresh temp root.
    fn dirs(root: &TempDir) -> PathBuf {
        let iteration_dir = root.path().join("iter");
        fs::create_dir_all(&iteration_dir).unwrap();
        iteration_dir
    }

    #[test]
    fn assembles_run_and_timing_for_every_task_from_disk() {
        let root = TempDir::new().unwrap();
        let iter = dirs(&root);
        let paths = write_iteration(
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
        write_claude_events(&paths[0].outputs_dir, "unused");
        write_claude_events(&paths[1].outputs_dir, "unused");

        let result = record_runs(&iter, Harness::ClaudeCode, false).unwrap();
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
        assert_eq!(timing["total_tokens"], json!(125));
        assert_eq!(timing["duration_ms"], json!(30_000));
        assert_eq!(timing["source"], json!("transcript"));
    }

    #[test]
    fn carries_run_index_from_dispatch_task_into_each_run_record() {
        let root = TempDir::new().unwrap();
        let iter = dirs(&root);
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

        let result = record_runs(&iter, Harness::Codex, false).unwrap();
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
        let iter = dirs(&root);
        let paths = write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: Some("Fixed it."),
            }],
        );
        write_codex_events(&paths[0].outputs_dir, "Codex final.");

        let result = record_runs(&iter, Harness::Codex, false).unwrap();
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
        let iter = dirs(&root);
        let paths = write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: None,
            }],
        );
        write_codex_events(&paths[0].outputs_dir, "Closing summary from Codex.");

        let result = record_runs(&iter, Harness::Codex, false).unwrap();
        assert_eq!(result.recorded, 1);
        assert_eq!(
            read_run(&iter, "crash", "with_skill").final_message,
            "Closing summary from Codex."
        );
    }

    #[test]
    fn skips_existing_run_without_overwrite_then_replaces_with_it() {
        let root = TempDir::new().unwrap();
        let iter = dirs(&root);
        let paths = write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: Some("New."),
            }],
        );
        write_claude_events(&paths[0].outputs_dir, "unused");
        let hand_written = json!({
            "eval_id": "crash", "condition": "with_skill",
            "skill_path": "/staged/skill/SKILL.md", "prompt": "Do the crash task",
            "files": [], "final_message": "Agent-authored.", "tool_invocations": []
        });
        fs::write(&paths[0].run_record_path, hand_written.to_string()).unwrap();

        let skipped = record_runs(&iter, Harness::ClaudeCode, false).unwrap();
        assert_eq!(skipped.recorded, 0);
        assert_eq!(skipped.skipped_existing, 1);
        assert_eq!(
            read_run(&iter, "crash", "with_skill").final_message,
            "Agent-authored."
        );

        let replaced = record_runs(&iter, Harness::ClaudeCode, true).unwrap();
        assert_eq!(replaced.recorded, 1);
        assert_eq!(read_run(&iter, "crash", "with_skill").final_message, "New.");
    }

    #[test]
    fn backfills_timing_only_when_absent() {
        let root = TempDir::new().unwrap();
        let iter = dirs(&root);
        let paths = write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: Some("Done."),
            }],
        );
        write_claude_events(&paths[0].outputs_dir, "unused");
        fs::write(
            &paths[0].timing_path,
            json!({"total_tokens": 12345, "duration_ms": 9000}).to_string(),
        )
        .unwrap();

        record_runs(&iter, Harness::ClaudeCode, false).unwrap();

        // Agent-captured completion-event timing wins; not overwritten.
        let timing = read_timing_value(&iter, "crash", "with_skill");
        assert_eq!(timing["total_tokens"], json!(12345));
        assert_eq!(timing["duration_ms"], json!(9000));
        assert!(timing.get("source").is_none());
    }

    #[test]
    fn skips_the_slot_entirely_when_no_final_message_source_exists() {
        let root = TempDir::new().unwrap();
        let iter = dirs(&root);
        write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: None,
            }],
        );
        // No final-message.md, no transcript.

        let result = record_runs(&iter, Harness::ClaudeCode, false).unwrap();
        assert_eq!(result.recorded, 0);
        assert_eq!(result.skipped_no_final_message, 1);
        assert!(!run_exists(&iter, "crash", "with_skill"));
        assert!(!timing_exists(&iter, "crash", "with_skill"));
    }

    #[test]
    fn writes_empty_invocations_and_no_timing_when_transcript_missing() {
        let root = TempDir::new().unwrap();
        let iter = dirs(&root);
        write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: Some("Done."),
            }],
        );
        // final-message.md exists but no events file is present.

        let result = record_runs(&iter, Harness::ClaudeCode, false).unwrap();
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
        let iter = dirs(&root);
        // Hand-authored/operator runs have no dispatch.json — the manual path owns them.
        let err = record_runs(&iter, Harness::ClaudeCode, false).unwrap_err();
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

    #[test]
    fn assembles_claude_records_from_each_tasks_events() {
        let root = TempDir::new().unwrap();
        let iter = dirs(&root);
        let paths = write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: Some("Fixed it."),
            }],
        );
        write_claude_events(&paths[0].outputs_dir, "Closing summary.");

        let result = record_runs(&iter, Harness::ClaudeCode, false).unwrap();
        assert_eq!(result.recorded, 1);
        assert_eq!(result.missing_transcript, 0);

        let run = read_run(&iter, "crash", "with_skill");
        // final-message.md wins when present.
        assert_eq!(run.final_message, "Fixed it.");
        assert_eq!(
            serde_json::to_value(&run.tool_invocations).unwrap(),
            json!([{"name": "Bash", "ordinal": 0, "args": {"command": "bun test"}, "result": "ok"}])
        );
        let timing = read_timing_value(&iter, "crash", "with_skill");
        assert_eq!(
            timing,
            json!({"total_tokens": 125, "duration_ms": 30_000, "source": "transcript"})
        );
    }

    #[test]
    fn falls_back_to_claude_result_final_text_when_final_message_md_missing() {
        // Claude `-p` has no --output-last-message, so the result event's text is
        // the primary final-message source.
        let root = TempDir::new().unwrap();
        let iter = dirs(&root);
        let paths = write_iteration(
            &iter,
            &[FixtureTask {
                eval_id: "crash",
                condition: "with_skill",
                final_message: None,
            }],
        );
        write_claude_events(&paths[0].outputs_dir, "Closing summary from claude -p.");

        let result = record_runs(&iter, Harness::ClaudeCode, false).unwrap();
        assert_eq!(result.recorded, 1);
        assert_eq!(
            read_run(&iter, "crash", "with_skill").final_message,
            "Closing summary from claude -p."
        );
    }

    #[test]
    fn claude_warning_points_at_events_file() {
        let result = RecordRunsResult {
            recorded: 2,
            missing_transcript: 2,
            ..Default::default()
        };
        let warning = result.transcript_warning(Harness::ClaudeCode).unwrap();
        assert!(
            warning.contains("claude-events.jsonl"),
            "names the Claude CLI events source: {warning}"
        );
        assert!(
            !warning.contains("agent_description"),
            "CLI dispatch doesn't use agent_description: {warning}"
        );
    }
}
