//! Stage 1 — `record-runs`.
//!
//! Assembles a schema-valid `run.json` (and
//! backfills `timing.json`) for every task in the iteration's `dispatch.json`,
//! from sources already on disk: carry-over fields from the dispatch task,
//! runner-owned completion metadata from `conversation.json`, and assistant
//! messages, tools, final text, and tokens from transcripts under
//! `outputs/turn-N/<harness>-events.jsonl` according to the harness descriptor.
//! Duration comes first from the runner's monotonic measurement in
//! `conversation.json`; historical or externally produced completions fall
//! back to transcript-native duration when available.
//!
//! Existing records always win: a previously assembled `run.json` is skipped
//! without `overwrite`, and `timing.json` is backfill-only. Token and duration
//! provenance are recorded independently because transcript-normalized tokens
//! and runner-measured duration normally come from different sources.
//!
//! Harnesses whose captures identify refused tool calls also get the
//! iteration-level `permission-denials.json` written here (see
//! [`crate::pipeline::permission_denials`]); harnesses that cannot detect a
//! refusal get no file at all, so its absence never reads as "nothing refused".
//!
//! Two sub-concerns live beside this module: [`conversation`] assembles a
//! task's ordered rounds, and [`prompt_read`] decides whether a
//! dispatch ever received its instructions.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::adapters::{TranscriptSummary, adapter_for};
use crate::core::fs::write_json;
use crate::core::{
    CodebaseRecord, ConditionSkill, ConversationEvent, ConversationRecord, Harness, RunRecord,
    SessionMode, SkillSource, TimingRecord, TimingSource,
};
use crate::pipeline::error::PipelineError;
use crate::pipeline::permission_denials::{self, CapturedDenial, TaskPermissionDenials};
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
    #[serde(default)]
    staged_skill_path: Option<String>,
    #[serde(default)]
    skills: Option<Vec<ConditionSkill>>,
    user_prompt: String,
    #[serde(alias = "fixtures")]
    files: Vec<String>,
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
    skill_source: Option<SkillSource>,
}

/// Tally of what record-runs did across the dispatch's tasks.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecordRunsResult {
    pub recorded: usize,
    pub skipped_existing: usize,
    pub skipped_no_final_response: usize,
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
    /// A loud, actionable warning when a task lacks one or more raw round
    /// transcripts. A task with no recoverable final response is skipped;
    /// partial multi-round evidence can still be recorded without complete
    /// timing or tool evidence.
    pub fn transcript_warning(&self, harness: Harness) -> Option<String> {
        if self.missing_transcript == 0 {
            return None;
        }
        let n = self.missing_transcript;
        let plural = if n == 1 { "" } else { "s" };
        let file = adapter_for(harness)
            .cli_events_filename()
            .unwrap_or_else(|| "the events file".to_string());
        let cause =
            format!("expected `{file}` transcript file(s) were not found under task outputs");
        Some(format!(
            "⚠ {n} task{plural} missing transcript evidence — {cause}; a task with no final \
             response was skipped, while partial multi-round evidence is recorded without \
             complete timing or tool evidence. Re-dispatch the affected task{plural}."
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

    /// Warn when a task never produced its runner-owned completion artifact.
    /// Raw per-turn transcripts are intentionally not ingested without it
    /// because the driver may have failed between rounds.
    pub fn incomplete_conversation_warning(&self) -> Option<String> {
        let n = self.skipped_incomplete_conversation;
        if n == 0 {
            return None;
        }
        let plural = if n == 1 { "" } else { "s" };
        Some(format!(
            "⚠ {n} task{plural} skipped — conversation.json is missing, so \
             eval-magic cannot distinguish a completed/stopped scenario from an interrupted \
             dispatch. Re-run `eval-magic dispatch` — it retries exactly the tasks with no \
             completion artifact."
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
             supports runner-built iterations. Re-run `eval-magic run` to prepare the campaign.",
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
        let Some(completion) = conversation::for_task(task)? else {
            result.skipped_incomplete_conversation += 1;
            continue;
        };
        let evidence = conversation::evidence_for_task(harness, task, &completion);
        let summary = evidence.summary.as_ref();
        if summary.is_none() || !evidence.transcripts_complete {
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
                permission_denials_for_task(harness, task, &completion),
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
                rounds: session_surfaces_for_task(harness, task, &completion),
            });
        }

        let run_record_path = Path::new(&task.run_record_path);
        if run_record_path.exists() && !overwrite {
            // A prior ingest already wrote this run.json — leave it untouched.
            result.skipped_existing += 1;
        } else if let Some(summary) = summary
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
            let final_message = summary.and_then(|summary| summary.final_text.clone());
            let Some(final_message) = final_message else {
                // No transcript text means there is no final response to grade.
                result.skipped_no_final_response += 1;
                continue;
            };

            let record = RunRecord {
                eval_id: task.eval_id.clone(),
                condition: task.condition.clone(),
                skill_path: task.skill_path.clone(),
                staged_skill_path: task.staged_skill_path.clone(),
                skills: task.skills.clone(),
                prompt: task.user_prompt.clone(),
                files: task.files.clone(),
                final_message,
                tool_invocations: summary
                    .map(|summary| summary.tool_invocations.clone())
                    .unwrap_or_default(),
                // Timing lives in timing.json; run.json never carries it.
                total_tokens: None,
                duration_ms: None,
                run_index: task.run_index,
                conversation: Some(evidence.conversation.clone()),
                codebase: task.codebase.clone(),
                skill_source: task.skill_source.clone(),
            };
            validate_against_schema::<RunRecord>(
                SchemaName::RunRecord,
                &serde_json::to_value(&record)?,
                &task.run_record_path,
            )?;
            write_json(run_record_path, &record)?;
            result.recorded += 1;
        }

        // timing.json remains backfill-only. Runner duration and transcript
        // tokens have independent provenance; an existing live-capture record
        // still wins unless the operator explicitly requests an overwrite.
        let timing_path = Path::new(&task.timing_path);
        if (!timing_path.exists() || overwrite)
            && let Some(timing) =
                timing_for_task(&completion, summary, evidence.transcripts_complete)
        {
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

fn timing_for_task(
    completion: &ConversationRecord,
    summary: Option<&TranscriptSummary>,
    transcripts_complete: bool,
) -> Option<TimingRecord> {
    let complete_summary = transcripts_complete.then_some(summary).flatten();
    let total_tokens = complete_summary.map(|summary| summary.total_tokens);
    let token_source = complete_summary.map(|_| TimingSource::Transcript);
    let (duration_ms, duration_source) = if let Some(duration_ms) = completion.duration_ms {
        (Some(Some(duration_ms)), Some(TimingSource::Runner))
    } else if let Some(summary) = complete_summary {
        (Some(summary.duration_ms), Some(TimingSource::Transcript))
    } else {
        (None, None)
    };

    (total_tokens.is_some() || duration_ms.is_some()).then_some(TimingRecord {
        total_tokens,
        duration_ms,
        token_source,
        duration_source,
    })
}

/// The skill/plugin surface each of a task's rounds reported. Unlike refusals,
/// these are kept per round rather than flattened: isolation has to hold for the
/// initial dispatch and every resumed turn, so a round whose transcript is
/// missing or silent stays a `None` that marks the task unproven.
fn session_surfaces_for_task(
    harness: Harness,
    task: &DispatchTask,
    conversation: &ConversationRecord,
) -> Vec<RoundSurface> {
    let adapter = adapter_for(harness);
    let Some(filename) = adapter.cli_events_filename() else {
        return Vec::new();
    };
    let outputs_dir = Path::new(&task.outputs_dir);
    let paths: Vec<PathBuf> = (1..=conversation.delivered_followups.saturating_add(1))
        .map(|round| outputs_dir.join(format!("turn-{round}")).join(&filename))
        .collect();
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
/// per-round events files, each denial paired with the phase of the round it
/// came from. A missing or unparseable transcript contributes nothing — absence
/// of evidence is not a denial.
fn permission_denials_for_task(
    harness: Harness,
    task: &DispatchTask,
    conversation: &ConversationRecord,
) -> Vec<CapturedDenial> {
    let adapter = adapter_for(harness);
    let Some(filename) = adapter.cli_events_filename() else {
        return Vec::new();
    };
    let outputs_dir = Path::new(&task.outputs_dir);
    (1..=conversation.delivered_followups.saturating_add(1))
        .map(|round| {
            let path = outputs_dir.join(format!("turn-{round}")).join(&filename);
            (round, path)
        })
        .filter(|(_, path)| path.exists())
        .flat_map(|(round, path)| {
            let plan_phase = round_ran_in_plan_mode(conversation, round);
            adapter
                .parse_permission_denials(&path)
                .into_iter()
                .flatten()
                .map(move |denial| CapturedDenial { denial, plan_phase })
        })
        .collect()
}

/// Whether `round` ran in the harness's plan mode, per the user message that
/// opened it. Absent modes mean the whole run was act mode.
fn round_ran_in_plan_mode(conversation: &ConversationRecord, round: u32) -> bool {
    conversation.events.iter().any(|event| {
        matches!(
            event,
            ConversationEvent::UserMessage {
                round: event_round,
                mode: Some(SessionMode::Plan),
                ..
            } if *event_round == round
        )
    })
}

#[cfg(test)]
mod tests;
