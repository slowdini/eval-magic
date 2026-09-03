//! Runner-owned execution of one dispatched task.
//!
//! Given a frozen harness descriptor and one task, this starts a native session,
//! gates and delivers each canned user follow-up a scripted task declares, and
//! writes the ordered `conversation.json` completion artifact ingest reads. A
//! one-shot task takes the same path with no follow-ups to deliver.
//!
//! Loading the plan these tasks come from belongs to [`super::drive`].

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};

use crate::adapters::cli_command::shell_quote_arg;
use crate::adapters::descriptor::PlanFileSection;
use crate::adapters::descriptor::subst;
use crate::adapters::descriptor_adapter::DescriptorAdapter;
use crate::adapters::harness::HarnessAdapter;
use crate::adapters::transcript::TranscriptSummary;
use crate::core::fs::artifact_path;
use crate::core::{
    ConversationEvent, ConversationRecord, ConversationStatus, ConversationStopReason, PlanRecord,
    ResponderOutcome, ResponderStopCause, RunnerTurn, SessionMode, ShellOutcome, TurnOrigin,
    run_in_posix_shell,
};
use crate::validation::{SchemaName, validate_against_schema};

use super::dispatch::DispatchTask;
use plan_phase::{PLAN_APPROVAL_PROMPT, PlanDecision, PresentedPlan};
use responder::{Consultation, ResponderRuntime};
use turn_plan::{NextTurn, TurnPlan};

mod plan_phase;
mod responder;
mod turn_plan;

/// How one dispatched task ended. A failure is not represented here — it stays
/// an `Err`, which the batch driver records per task rather than propagating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOutcome {
    Completed {
        delivered_followups: u32,
        source: TurnSource,
        /// The round the runner's plan approval opened, for a plan-mode task.
        plan_approved_in_round: Option<u32>,
    },
    Stopped {
        before_followup: u32,
        /// Always present in practice — the schema requires a stop reason on a
        /// stopped conversation — but carried as written rather than filled in
        /// with a guess, so an outcome can never name the wrong reason.
        reason: Option<ConversationStopReason>,
        /// Why the responder produced no usable reply, when it was the
        /// responder that stopped the run. An honest refusal and a broken
        /// dispatch end the run identically, so the warning has to say which.
        cause: Option<ResponderStopCause>,
    },
    TimedOut {
        round: u32,
    },
    SkippedExisting,
}

/// What produced a task's follow-up turns. Only used to word an outcome: a
/// scripted run and a responder-driven one stop for different reasons and an
/// operator reading the batch summary needs to know which they are looking at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnSource {
    Scripted,
    Responder,
}

impl TurnSource {
    fn noun(self) -> &'static str {
        match self {
            Self::Scripted => "scripted follow-up turn(s)",
            Self::Responder => "responder turn(s)",
        }
    }
}

impl TaskOutcome {
    /// The one-line human summary of this outcome.
    pub fn summary(&self) -> String {
        match self {
            Self::Completed {
                delivered_followups,
                plan_approved_in_round: Some(round),
                ..
            } => format!(
                "completed with {delivered_followups} follow-up turn(s), plan approved in round \
                 {round}"
            ),
            Self::Completed {
                delivered_followups: 0,
                ..
            } => "completed".to_string(),
            Self::Completed {
                delivered_followups,
                source,
                ..
            } => format!("completed with {delivered_followups} {}", source.noun()),
            Self::Stopped {
                before_followup,
                reason: Some(ConversationStopReason::PlanNotPresented),
                ..
            } => format!(
                "stopped before turn {before_followup} — the planning phase ended without a plan \
                 to approve"
            ),
            Self::Stopped {
                before_followup,
                reason: Some(ConversationStopReason::ResponderCannotAnswer),
                cause,
            } => format!(
                "stopped before turn {before_followup} — the responder produced no usable reply \
                 ({})",
                cause_label(*cause)
            ),
            Self::Stopped {
                before_followup,
                reason: Some(ConversationStopReason::MaxTurnsReached),
                ..
            } => format!(
                "stopped at the responder's max_turns bound after {} turn(s)",
                before_followup.saturating_sub(1)
            ),
            Self::Stopped {
                before_followup, ..
            } => format!("stopped before scripted follow-up {before_followup}"),
            Self::TimedOut { round } => format!("timed out in round {round}"),
            Self::SkippedExisting => "skipped (already complete)".to_string(),
        }
    }
}

/// How a stop cause reads in a warning. Absent only for a stop the responder
/// did not produce, which no caller here words this way.
pub fn cause_label(cause: Option<ResponderStopCause>) -> &'static str {
    cause.map_or("cause unrecorded", ResponderStopCause::wire_name)
}

/// The dispatch-wide settings every task shares, frozen in `dispatch.json` and
/// read back once by the batch driver. Grouped rather than passed one by one
/// because they travel together and always come from the same envelope.
#[derive(Debug, Clone, Copy)]
pub struct DispatchSettings<'a> {
    pub guard: bool,
    pub agent_model: Option<&'a str>,
    /// The model consulted after each round of a responder task. `None` runs the
    /// consultation on the harness's default model.
    pub responder_model: Option<&'a str>,
    pub agent_env: &'a BTreeMap<String, String>,
}

/// Execute one task: start a native session, deliver every scripted follow-up
/// whose gate is met, and write the `conversation.json` completion artifact.
pub fn run_task(
    adapter: &DescriptorAdapter,
    task: &DispatchTask,
    settings: &DispatchSettings<'_>,
    overwrite: bool,
    timeout: Option<Duration>,
) -> anyhow::Result<TaskOutcome> {
    let DispatchSettings {
        guard,
        agent_model,
        responder_model,
        agent_env,
    } = *settings;
    // One budget for the whole task, not per round: a scripted conversation is
    // a single dispatch from the operator's point of view.
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    // A one-shot task takes the same path with nothing to deliver, so the loop
    // below is the single delivery path for every shape of task.
    let plan = TurnPlan::for_task(task);
    let conversation_path = task
        .conversation_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("multi-turn task is missing conversation_path"))?;
    // A finished task is skipped rather than refused: a rerun of `dispatch`
    // is how an operator retries the failures in a batch, and the tasks that
    // already completed must not be redone on the way.
    if conversation_path.exists() && !overwrite {
        return Ok(TaskOutcome::SkippedExisting);
    }
    let eval_root = task
        .eval_root
        .as_deref()
        .ok_or_else(|| anyhow!("multi-turn task is missing eval_root"))?;
    let events_filename = adapter
        .cli_events_filename()
        .ok_or_else(|| anyhow!("harness declares no transcript events filename"))?;
    // Plan mode starts the session read-only and the approval resumes it, so
    // a plan-mode task needs the resume template even when it is otherwise
    // one-shot. Other one-shot tasks never resume, and a harness may support
    // them without declaring `[conversation]` at all (cline does).
    let plan_mode = task.plan_mode;
    let mut mode = if plan_mode {
        SessionMode::Plan
    } else {
        SessionMode::Act
    };
    let initial_template = adapter
        .cli_exec_command_in_mode(mode, guard, agent_model, agent_env)
        .ok_or_else(|| match mode {
            SessionMode::Plan => anyhow!("harness declares no plan-mode dispatch command"),
            SessionMode::Act => anyhow!("harness declares no initial dispatch command"),
        })?;
    let needs_resume = plan.delivers_followups() || plan_mode;
    let resume_templates = if needs_resume {
        Some(ResumeTemplates {
            act: adapter
                .cli_resume_command_in_mode(SessionMode::Act, guard, agent_model, agent_env)
                .ok_or_else(|| anyhow!("harness declares no native conversation resume command"))?,
            plan: plan_mode
                .then(|| {
                    adapter
                        .cli_resume_command_in_mode(
                            SessionMode::Plan,
                            guard,
                            agent_model,
                            agent_env,
                        )
                        .ok_or_else(|| anyhow!("harness declares no plan-mode resume command"))
                })
                .transpose()?,
        })
    } else {
        None
    };
    // The plan file only matters while planning; `home` resolves its `~`.
    let plan_file = plan_mode.then(|| adapter.plan_file()).flatten();
    let home = std::env::home_dir();
    if overwrite && conversation_path.exists() {
        fs::remove_file(&conversation_path).with_context(|| {
            format!(
                "failed to remove stale conversation artifact {}",
                conversation_path.display()
            )
        })?;
    }

    // Consultations run outside every task env, so nothing the responder does
    // reaches the codebase under measurement or picks up its `CLAUDE.md`.
    let responder_runtime = match &plan {
        TurnPlan::Responder(_) => Some(ResponderRuntime {
            adapter,
            model: responder_model,
            agent_env,
            responder_dir: PathBuf::from(
                task.responder_dir
                    .as_deref()
                    .ok_or_else(|| anyhow!("responder task is missing responder_dir"))?,
            ),
            deadline,
        }),
        TurnPlan::OneShot | TurnPlan::Scripted(_) => None,
    };

    let base_outputs = Path::new(&task.outputs_dir);
    let mut events = vec![ConversationEvent::UserMessage {
        ordinal: 0,
        round: 1,
        text: task.user_prompt.clone(),
        origin: None,
        mode: plan_mode.then_some(mode),
    }];
    let first_outputs = base_outputs.join("turn-1");
    let initial_command = render_command(
        &initial_template,
        eval_root,
        &task.dispatch_prompt_path,
        &first_outputs,
        None,
        None,
        1,
    );
    if execute_round(
        &initial_command,
        Path::new(eval_root),
        &first_outputs,
        agent_env,
        1,
        deadline,
    )? == RoundOutcome::TimedOut
    {
        // Turn 1 never answered, so there is no transcript to parse and no
        // session to resume. The seeded user message is the whole record.
        return write_conversation(
            &conversation_path,
            ConversationRecord {
                status: ConversationStatus::TimedOut,
                delivered_followups: 0,
                stop_reason: None,
                stopped_before_followup: None,
                timed_out_in_round: Some(1),
                events,
                responder_outcome: None,
                plan: None,
            },
            plan.source(),
        );
    }
    let first_summary = parse_round(adapter, &first_outputs, &events_filename, 1)?;
    let session_id = if needs_resume {
        Some(
            first_summary
                .session_id
                .clone()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| anyhow!("turn 1 transcript did not expose a native session id"))?,
        )
    } else {
        None
    };
    let mut presented = plan_hit(
        &first_summary,
        mode,
        plan_file.as_ref(),
        home.as_deref(),
        Path::new(eval_root),
    );
    let mut preceding_assistant = round_text(1, &first_summary, presented.as_ref())?;
    let mut final_message = preceding_assistant.clone();
    let mut last_round = 1_u32;

    let mut delivered_followups = 0_u32;
    // Follow-ups the planning phase consumed — responder answers and the
    // approval — which a script must not count as its own deliveries.
    let mut plan_phase_followups = 0_u32;
    let mut plan_record: Option<PlanRecord> = None;
    let mut stop_reason = None;
    let mut stopped_before_followup = None;
    let mut timed_out_in_round = None;
    let mut responder_outcome: Option<ResponderOutcome> = None;
    // Every reply the responder has produced, in order: the prompt for the next
    // consultation, and the repeat guard's memory.
    let mut responder_replies: Vec<String> = Vec::new();

    loop {
        let followup = delivered_followups.saturating_add(1);
        let consultation = Consultation {
            task_prompt: &task.user_prompt,
            prior_replies: &responder_replies,
            final_message: &final_message,
            planning: mode == SessionMode::Plan,
        };
        let previous_reply = responder_replies.last().map(String::as_str);
        let (prompt, origin, next_mode) = if mode == SessionMode::Plan {
            let responder = match &plan {
                TurnPlan::Responder(policy) => Some((
                    *policy,
                    responder_runtime
                        .as_ref()
                        .expect("a responder plan resolved its runtime alongside the plan itself"),
                )),
                TurnPlan::OneShot | TurnPlan::Scripted(_) => None,
            };
            match plan_phase::decide(
                presented.take(),
                responder,
                followup,
                &consultation,
                previous_reply,
            ) {
                PlanDecision::Approve(approved) => {
                    let plan_path = base_outputs.join("plan.md");
                    write_atomic(&plan_path, approved.text.clone())?;
                    plan_record = Some(PlanRecord {
                        presented_in_round: last_round,
                        approved_in_round: last_round.saturating_add(1),
                        signal: approved.signal,
                        artifact_path: Some(artifact_path(&plan_path)),
                    });
                    (
                        PLAN_APPROVAL_PROMPT.to_string(),
                        Some(TurnOrigin::Runner {
                            runner: RunnerTurn::PlanApproval,
                        }),
                        SessionMode::Act,
                    )
                }
                PlanDecision::Answer { text, origin } => {
                    responder_replies.push(text.clone());
                    (text, Some(origin), SessionMode::Plan)
                }
                PlanDecision::Stop { reason, responder } => {
                    stop_reason = Some(reason);
                    stopped_before_followup = Some(followup);
                    responder_outcome = responder;
                    break;
                }
            }
        } else {
            let next = plan.next_turn(
                delivered_followups.saturating_sub(plan_phase_followups),
                followup,
                &preceding_assistant,
                &consultation,
                responder_runtime.as_ref(),
                previous_reply,
            )?;
            match next {
                NextTurn::Done { responder } => {
                    responder_outcome = responder;
                    break;
                }
                NextTurn::Stop { reason, responder } => {
                    stop_reason = Some(reason);
                    stopped_before_followup = Some(followup);
                    responder_outcome = responder;
                    break;
                }
                NextTurn::Deliver { text, origin } => {
                    if origin.is_some() {
                        responder_replies.push(text.clone());
                    }
                    (text, origin, SessionMode::Act)
                }
            }
        };
        if mode == SessionMode::Plan {
            plan_phase_followups = plan_phase_followups.saturating_add(1);
        }
        mode = next_mode;

        let round = followup.saturating_add(1);
        events.push(ConversationEvent::UserMessage {
            ordinal: events.len() as u32,
            round,
            text: prompt.clone(),
            origin,
            mode: plan_mode.then_some(mode),
        });
        delivered_followups = delivered_followups.saturating_add(1);

        let round_outputs = base_outputs.join(format!("turn-{round}"));
        let templates = resume_templates
            .as_ref()
            .expect("a task that delivers follow-ups resolved its resume templates above");
        let resume_template = match mode {
            SessionMode::Act => templates.act.as_str(),
            SessionMode::Plan => templates
                .plan
                .as_deref()
                .expect("a plan-mode task resolved its plan-mode resume template above"),
        };
        let command = render_command(
            resume_template,
            eval_root,
            &task.dispatch_prompt_path,
            &round_outputs,
            Some(
                session_id
                    .as_deref()
                    .expect("a follow-up task resolved a session id above"),
            ),
            Some(&prompt),
            round,
        );
        if execute_round(
            &command,
            Path::new(eval_root),
            &round_outputs,
            agent_env,
            round,
            deadline,
        )? == RoundOutcome::TimedOut
        {
            timed_out_in_round = Some(round);
            break;
        }
        let summary = parse_round(adapter, &round_outputs, &events_filename, round)?;
        if let Some(observed) = summary.session_id.as_deref()
            && Some(observed) != session_id.as_deref()
        {
            bail!(
                "turn {round} resumed session {observed:?}, expected {:?}",
                session_id.as_deref().unwrap_or_default()
            );
        }
        presented = plan_hit(
            &summary,
            mode,
            plan_file.as_ref(),
            home.as_deref(),
            Path::new(eval_root),
        );
        preceding_assistant = round_text(round, &summary, presented.as_ref())?;
        final_message = preceding_assistant.clone();
        last_round = round;
    }

    // A timeout outranks a gate stop: the conversation was cut short, so what
    // the last round would have gated on was never observed.
    let status = match (timed_out_in_round, stop_reason) {
        (Some(_), _) => ConversationStatus::TimedOut,
        (None, Some(_)) => ConversationStatus::Stopped,
        (None, None) => ConversationStatus::Completed,
    };
    write_conversation(
        &conversation_path,
        ConversationRecord {
            status,
            delivered_followups,
            stop_reason: timed_out_in_round.map_or(stop_reason, |_| None),
            stopped_before_followup: timed_out_in_round.map_or(stopped_before_followup, |_| None),
            timed_out_in_round,
            events,
            // A timeout outranks the responder's verdict for the same reason it
            // outranks a gate stop: the round it judged never finished.
            responder_outcome: timed_out_in_round.map_or(responder_outcome, |_| None),
            // An approval that happened stays recorded even if a later round
            // timed out: the plan was presented and approved as written.
            plan: plan_record,
        },
        plan.source(),
    )
}

/// The resume commands a task may need: act mode for every task that resumes,
/// plan mode only while a plan-mode task is still planning.
struct ResumeTemplates {
    act: String,
    plan: Option<String>,
}

/// The plan a plan-mode round presented through the harness's plan file, when
/// the round ran in plan mode and the harness declares such a file.
fn plan_hit(
    summary: &TranscriptSummary,
    mode: SessionMode,
    plan_file: Option<&PlanFileSection>,
    home: Option<&Path>,
    eval_root: &Path,
) -> Option<PresentedPlan> {
    if mode != SessionMode::Plan {
        return None;
    }
    plan_phase::plan_file_written(summary, plan_file?, home?, eval_root)
}

/// The round's final assistant message. A plan-mode round that ended by writing
/// its plan file may have no closing message; the plan then stands in for it.
fn round_text(
    round: u32,
    summary: &TranscriptSummary,
    presented: Option<&PresentedPlan>,
) -> anyhow::Result<String> {
    match final_text_for_round(round, summary) {
        Ok(text) => Ok(text),
        Err(error) => presented.map(|plan| plan.text.clone()).ok_or(error),
    }
}

/// Validate, commit, and report one task's completion artifact.
fn write_conversation(
    conversation_path: &Path,
    conversation: ConversationRecord,
    source: TurnSource,
) -> anyhow::Result<TaskOutcome> {
    let _: ConversationRecord = validate_against_schema(
        SchemaName::Conversation,
        &serde_json::to_value(&conversation)?,
        &conversation_path.to_string_lossy(),
    )?;
    write_json_atomic(conversation_path, &conversation)?;

    Ok(match conversation.status {
        ConversationStatus::Completed => TaskOutcome::Completed {
            delivered_followups: conversation.delivered_followups,
            source,
            plan_approved_in_round: conversation
                .plan
                .as_ref()
                .map(|plan| plan.approved_in_round),
        },
        ConversationStatus::Stopped => TaskOutcome::Stopped {
            before_followup: conversation.stopped_before_followup.unwrap_or_default(),
            reason: conversation.stop_reason,
            cause: conversation
                .responder_outcome
                .as_ref()
                .and_then(|outcome| outcome.cause),
        },
        ConversationStatus::TimedOut => TaskOutcome::TimedOut {
            round: conversation.timed_out_in_round.unwrap_or(1),
        },
    })
}

fn parse_round(
    adapter: &DescriptorAdapter,
    outputs_dir: &Path,
    events_filename: &str,
    round: u32,
) -> anyhow::Result<TranscriptSummary> {
    let path = outputs_dir.join(events_filename);
    adapter
        .parse_cli_events_full(&path)
        .with_context(|| format!("failed to parse turn {round} transcript {}", path.display()))
}

fn final_text_for_round(round: u32, summary: &TranscriptSummary) -> anyhow::Result<String> {
    summary
        .final_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("turn {round} transcript did not contain a final assistant message"))
}

/// Render a one-shot dispatch command: the exec template with its task
/// placeholders bound and no session to resume. Judge dispatch uses this too,
/// binding the iteration directory and the judge prompt.
pub fn render_dispatch_command(
    template: &str,
    eval_root: &str,
    dispatch_prompt_path: &str,
    outputs_dir: &Path,
) -> String {
    render_command(
        template,
        eval_root,
        dispatch_prompt_path,
        outputs_dir,
        None,
        None,
        1,
    )
}

fn render_command(
    template: &str,
    eval_root: &str,
    dispatch_prompt_path: &str,
    outputs_dir: &Path,
    session_id: Option<&str>,
    prompt: Option<&str>,
    round: u32,
) -> String {
    let eval_root = shell_quote_arg(eval_root);
    let prompt_path = shell_quote_arg(dispatch_prompt_path);
    let outputs_dir = shell_quote_arg(&outputs_dir.to_string_lossy());
    let session_arg = shell_quote_arg(session_id.unwrap_or_default());
    let prompt_arg = shell_quote_arg(prompt.unwrap_or_default());
    let round = round.to_string();
    subst(
        &template
            .replace("<eval-root>", &eval_root)
            .replace("<dispatch_prompt_path>", &prompt_path)
            .replace("<outputs_dir>", &outputs_dir)
            .replace("<round>", &round),
        &[("session_arg", &session_arg), ("prompt_arg", &prompt_arg)],
    )
}

/// Run one round's harness command. `deadline` is the whole task's, not this
/// round's: a scripted conversation is one dispatch from the operator's point
/// of view, so its budget spans every turn it delivers.
///
/// A round that outruns the deadline returns `Ok(RoundOutcome::TimedOut)`. That
/// is a recorded result, unlike a nonzero exit, which is a failure.
fn execute_round(
    command: &str,
    eval_root: &Path,
    outputs_dir: &Path,
    agent_env: &BTreeMap<String, String>,
    round: u32,
    deadline: Option<Instant>,
) -> anyhow::Result<RoundOutcome> {
    fs::create_dir_all(outputs_dir)
        .with_context(|| format!("failed to create turn {round} outputs"))?;
    // Saturating: a deadline already passed leaves zero budget, so a turn that
    // cannot finish is not begun.
    let remaining = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
    let outcome = run_in_posix_shell(command, eval_root, agent_env, remaining)
        .map_err(|message| anyhow!("turn {round}: {message}"))?;
    match outcome {
        ShellOutcome::Exited(status) if status.success() => Ok(RoundOutcome::Completed),
        ShellOutcome::Exited(status) => {
            bail!("harness command for turn {round} exited with {status}")
        }
        ShellOutcome::TimedOut => Ok(RoundOutcome::TimedOut),
    }
}

/// How one round's harness command ended, once a nonzero exit has been ruled
/// out by [`execute_round`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoundOutcome {
    Completed,
    TimedOut,
}

fn write_json_atomic(path: &Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
    let mut body = serde_json::to_string_pretty(value)?;
    body.push('\n');
    write_atomic(path, body)
}

/// Write `body` to `path` through a sibling temp file, so a reader never sees a
/// half-written artifact.
fn write_atomic(path: &Path, body: String) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("artifact path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let temp_path = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact")
    ));
    fs::write(&temp_path, body)?;
    fs::rename(&temp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{execute_round, final_text_for_round};
    use crate::adapters::TranscriptSummary;
    use crate::adapters::cli_command::shell_quote_arg;

    #[test]
    fn execute_round_creates_the_round_output_directory_before_shell_redirection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outputs = tmp.path().join("outputs").join("turn-1");
        let events = outputs.join("events.jsonl");
        let command = format!(
            "printf '%s\\n' '{{\"type\":\"done\"}}' > {}",
            shell_quote_arg(&events.to_string_lossy())
        );

        execute_round(&command, tmp.path(), &outputs, &BTreeMap::new(), 1, None).unwrap();

        assert_eq!(
            std::fs::read_to_string(events).unwrap(),
            "{\"type\":\"done\"}\n"
        );
    }

    #[test]
    fn final_text_for_round_returns_the_parser_result() {
        let summary = TranscriptSummary {
            tool_invocations: Vec::new(),
            events: Vec::new(),
            session_id: Some("session-1".into()),
            total_tokens: None,
            duration_ms: None,
            final_text: Some("Which timezone?\nPlease include the locale.".into()),
        };
        let assistant = final_text_for_round(1, &summary).unwrap();

        assert_eq!(assistant, "Which timezone?\nPlease include the locale.");
    }
}
