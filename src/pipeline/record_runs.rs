//! Stage 1 — `record-runs`.
//!
//! Assembles a schema-valid `run.json` (and
//! backfills `timing.json`) for every task in the iteration's `dispatch.json`,
//! from sources already on disk: carry-over fields from the dispatch task, the
//! `final_message` (from `<outputs_dir>/final-message.md`, falling back to the
//! transcript's last assistant text), and `tool_invocations`/tokens/duration from
//! each task's events file. Scripted tasks take ordered messages/tools from the
//! runner-owned `conversation.json` and combine raw timing across
//! `outputs/turn-N/<harness>-events.jsonl` according to the harness descriptor.
//!
//! Existing records always win: an agent/operator-written `run.json` is skipped
//! without `overwrite`, and `timing.json` is backfill-only — completion-event
//! numbers captured at dispatch time are never replaced by transcript-derived
//! ones (whose accounting is harness-specific and may not be comparable 1:1).
//!
//! Harnesses whose captures identify refused tool calls also get the
//! iteration-level `permission-denials.json` written here (see
//! [`crate::pipeline::permission_denials`]); harnesses that cannot detect a
//! refusal get no file at all, so its absence never reads as "nothing refused".
//!
//! Two sub-concerns live beside this module: [`conversation`] assembles a
//! scripted task's ordered rounds, and [`prompt_read`] decides whether a
//! dispatch ever received its instructions.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::adapters::{PermissionDenial, TranscriptSummary, adapter_for};
use crate::core::fs::write_json;
use crate::core::{
    CodebaseRecord, ConversationEvent, ConversationRecord, Harness, RunRecord, TimingRecord,
    TimingSource,
};
use crate::pipeline::error::PipelineError;
use crate::pipeline::permission_denials::{self, TaskPermissionDenials};
use crate::pipeline::session_surface::{self, RoundSurface, TaskSessionSurface};
use crate::pipeline::shadow_verification;
use crate::validation::{SchemaName, validate_against_schema};
use prompt_read::{prompt_read_failed, prompt_sentinel};

mod conversation;
mod prompt_read;

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
    #[serde(default)]
    conversation_path: Option<String>,
    /// Group this task belongs to; absent for a single-group run. Carried so the
    /// session-surface report can be joined back to the comparison cells a
    /// shadow finding names.
    #[serde(default)]
    group: Option<String>,
    /// The codebase the environment was built from, copied through to the run
    /// record so grading can name the tree a result came from.
    #[serde(default)]
    codebase: Option<CodebaseRecord>,
}

/// Tally of what record-runs did across the dispatch's tasks.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecordRunsResult {
    pub recorded: usize,
    pub skipped_existing: usize,
    pub skipped_no_final_message: usize,
    pub missing_transcript: usize,
    pub skipped_prompt_unread: usize,
    pub skipped_incomplete_conversation: usize,
    /// Tool calls the harness refused, excluding the write guard's own blocks
    /// (`guard-denials.json` reports those). Always 0 for a harness whose
    /// transcript cannot identify a refusal.
    pub permission_denials: usize,
    /// How many tasks those denials are spread across.
    pub permission_denial_tasks: usize,
    /// Dispatches that reported which skills and plugins they could see, for
    /// every round. Always 0 for a harness whose transcript carries no such
    /// roster, in which case no `session-surface.json` is written at all.
    pub dispatches_with_surface: usize,
    /// Dispatches whose transcripts left the surface unknown for at least one
    /// round. Shadow findings covering those cells stay unverified.
    pub dispatches_without_surface: usize,
}

impl RecordRunsResult {
    /// A loud, actionable warning when one-shot events are missing or a
    /// scripted conversation lacks one or more raw round transcripts. Scripted
    /// runs retain their ordered conversation evidence, but timing is omitted
    /// unless every round can be combined.
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
            .unwrap_or_else(|| "the events file".to_string());
        let cause =
            format!("expected `{file}` transcript file(s) were not found under task outputs");
        Some(format!(
            "{lead} — {cause}; one-shot runs lack tool evidence and timing, while scripted runs \
             retain conversation evidence but omit incomplete timing."
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

    /// A loud, actionable warning when the harness refused one or more tool
    /// calls. Nothing failed and nothing was skipped — the agent simply could
    /// not do what it tried, which commonly turns a run that should have
    /// executed something into static reasoning. `None` when none were refused.
    pub fn permission_denial_warning(&self) -> Option<String> {
        if self.permission_denials == 0 {
            return None;
        }
        let n = self.permission_denials;
        let plural = if n == 1 { "" } else { "s" };
        let tasks = self.permission_denial_tasks;
        let task_plural = if tasks == 1 { "" } else { "s" };
        Some(format!(
            "⚠ {n} tool call{plural} across {tasks} task{task_plural} were permission-denied — the \
             agent's attempt was refused, so its behavior changed and the run may have degraded to \
             static reasoning. Inspect permission-denials.json before trusting those data points; \
             `aggregate` also flags them in validity_warnings."
        ))
    }

    /// Warn when a scripted task never produced its runner-owned completion
    /// artifact. Raw per-turn transcripts are intentionally not ingested
    /// without it because the driver may have failed between turns.
    pub fn incomplete_conversation_warning(&self) -> Option<String> {
        let n = self.skipped_incomplete_conversation;
        if n == 0 {
            return None;
        }
        let plural = if n == 1 { "" } else { "s" };
        Some(format!(
            "⚠ {n} scripted conversation{plural} skipped — conversation.json is missing, so \
             eval-magic cannot distinguish a completed/stopped scenario from an interrupted \
             dispatch. Re-run the corresponding `dispatch-task` command."
        ))
    }
}

/// Assemble `run.json` + `timing.json` for every task in
/// `<iteration_dir>/dispatch.json`. See the module docs for the field sources and
/// the existing-record precedence rules.
pub fn record_runs(
    iteration_dir: &Path,
    iteration: u32,
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
    let detects_denials = adapter_for(harness).surfaces_permission_denials();
    let reports_surface = adapter_for(harness).surfaces_session_surface();
    let mut denial_tasks: Vec<TaskPermissionDenials> = Vec::new();
    let mut surface_tasks: Vec<TaskSessionSurface> = Vec::new();
    for task in &tasks {
        let conversation = conversation::for_task(task)?;
        if task.conversation_path.is_some() && conversation.is_none() {
            result.skipped_incomplete_conversation += 1;
            continue;
        }
        let (summary, transcripts_complete) = match &conversation {
            Some(conversation) => conversation::summary_for_task(harness, task, conversation),
            None => (transcript_summary_for_task(harness, task), true),
        };
        if summary.is_none() || !transcripts_complete {
            result.missing_transcript += 1;
        }

        // Collected before the record/timing skips below: a refused tool call is
        // worth reporting even for a task that never becomes a graded data point.
        // (A scripted task missing conversation.json is the one exception — it
        // returned above, since without it the rounds that ran are unknown.)
        if detects_denials {
            let denials = TaskPermissionDenials::new(
                task.eval_id.clone(),
                task.condition.clone(),
                task.run_index,
                permission_denials_for_task(harness, task, conversation.as_ref()),
            );
            if denials.harness_denial_count() > 0 {
                result.permission_denials += denials.harness_denial_count();
                result.permission_denial_tasks += 1;
            }
            denial_tasks.push(denials);
        }

        // Collected here for the same reason as the denials above: a task that
        // never becomes a graded data point still carries evidence about which
        // live sources its dispatches could see.
        if reports_surface {
            surface_tasks.push(TaskSessionSurface {
                eval_id: task.eval_id.clone(),
                condition: task.condition.clone(),
                run_index: task.run_index,
                group: task.group.clone(),
                rounds: session_surfaces_for_task(harness, task, conversation.as_ref()),
            });
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
            let final_message = if let Some(conversation) = &conversation {
                conversation
                    .events
                    .iter()
                    .rev()
                    .find_map(|event| match event {
                        ConversationEvent::AssistantMessage { text, .. } => Some(text.clone()),
                        _ => None,
                    })
            } else if final_message_path.exists() {
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
                tool_invocations: conversation.as_ref().map_or_else(
                    || {
                        summary
                            .as_ref()
                            .map(|s| s.tool_invocations.clone())
                            .unwrap_or_default()
                    },
                    conversation::tool_invocations,
                ),
                // Timing lives in timing.json; run.json never carries it.
                total_tokens: None,
                duration_ms: None,
                run_index: task.run_index,
                conversation: conversation.clone(),
                codebase: task.codebase.clone(),
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
        if transcripts_complete
            && let Some(summary) = &summary
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

    // Only written when the harness can actually detect a refusal, so a missing
    // file always means "not detected" rather than "nothing was refused".
    if detects_denials {
        permission_denials::write_report(iteration_dir, iteration, denial_tasks)?;
    }

    // Same contract: absent means "this harness cannot report a surface", which
    // leaves shadow findings unverified rather than refuted.
    if reports_surface {
        let report = session_surface::write_report(iteration_dir, iteration, surface_tasks)?;
        result.dispatches_with_surface = report.tasks_with_evidence;
        result.dispatches_without_surface = report.tasks_without_evidence;
        // Resolve the preflight's findings now that there is evidence to resolve
        // them against, so the verdict is persisted rather than recomputed.
        shadow_verification::verify_iteration(iteration_dir)?;
    }

    Ok(result)
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

/// The skill/plugin surface each of a task's rounds reported. Unlike refusals,
/// these are kept per round rather than flattened: isolation has to hold for the
/// initial dispatch and every resumed turn, so a round whose transcript is
/// missing or silent stays a `None` that marks the task unproven.
fn session_surfaces_for_task(
    harness: Harness,
    task: &DispatchTask,
    conversation: Option<&ConversationRecord>,
) -> Vec<RoundSurface> {
    let adapter = adapter_for(harness);
    let Some(filename) = adapter.cli_events_filename() else {
        return Vec::new();
    };
    let outputs_dir = Path::new(&task.outputs_dir);
    let paths: Vec<PathBuf> = match conversation {
        Some(conversation) => (1..=conversation.delivered_followups.saturating_add(1))
            .map(|round| outputs_dir.join(format!("turn-{round}")).join(&filename))
            .collect(),
        None => vec![outputs_dir.join(&filename)],
    };
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| RoundSurface {
            round: index as u32 + 1,
            surface: path
                .exists()
                .then(|| adapter.parse_session_surface(path).ok().flatten())
                .flatten(),
        })
        .collect()
}

/// The tool calls the harness refused across a task's transcript(s): the
/// one-shot events file, or every scripted round's, since each round is its own
/// CLI invocation with its own refusals. A missing or unparseable transcript
/// contributes nothing — absence of evidence is not a denial.
fn permission_denials_for_task(
    harness: Harness,
    task: &DispatchTask,
    conversation: Option<&ConversationRecord>,
) -> Vec<PermissionDenial> {
    let adapter = adapter_for(harness);
    let Some(filename) = adapter.cli_events_filename() else {
        return Vec::new();
    };
    let outputs_dir = Path::new(&task.outputs_dir);
    let paths: Vec<PathBuf> = match conversation {
        Some(conversation) => (1..=conversation.delivered_followups.saturating_add(1))
            .map(|round| outputs_dir.join(format!("turn-{round}")).join(&filename))
            .collect(),
        None => vec![outputs_dir.join(&filename)],
    };
    paths
        .iter()
        .filter(|path| path.exists())
        .filter_map(|path| adapter.parse_permission_denials(path).ok())
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests;
