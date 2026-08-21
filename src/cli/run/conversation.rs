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
use crate::adapters::descriptor::subst;
use crate::adapters::descriptor_adapter::DescriptorAdapter;
use crate::adapters::harness::HarnessAdapter;
use crate::adapters::transcript::{TranscriptEvent, TranscriptSummary};
use crate::core::{
    ConversationEvent, ConversationRecord, ConversationStatus, ConversationStopReason,
    ShellOutcome, run_in_posix_shell,
};
use crate::validation::{SchemaName, validate_against_schema};

use super::dispatch::DispatchTask;
use turn_plan::{NextTurn, TurnPlan};

mod responder;
mod turn_plan;

/// How one dispatched task ended. A failure is not represented here — it stays
/// an `Err`, which the batch driver records per task rather than propagating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOutcome {
    Completed {
        delivered_followups: u32,
        source: TurnSource,
    },
    Stopped {
        before_followup: u32,
        /// Always present in practice — the schema requires a stop reason on a
        /// stopped conversation — but carried as written rather than filled in
        /// with a guess, so an outcome can never name the wrong reason.
        reason: Option<ConversationStopReason>,
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
                delivered_followups: 0,
                ..
            } => "completed".to_string(),
            Self::Completed {
                delivered_followups,
                source,
            } => format!("completed with {delivered_followups} {}", source.noun()),
            Self::Stopped {
                before_followup,
                reason: Some(ConversationStopReason::ResponderCannotAnswer),
            } => format!(
                "stopped before turn {before_followup} — the responder could not answer the \
                 agent's question"
            ),
            Self::Stopped {
                before_followup,
                reason: Some(ConversationStopReason::MaxTurnsReached),
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

/// Execute one task: start a native session, deliver every scripted follow-up
/// whose gate is met, and write the `conversation.json` completion artifact.
pub fn run_task(
    adapter: &DescriptorAdapter,
    task: &DispatchTask,
    guard: bool,
    agent_model: Option<&str>,
    agent_env: &BTreeMap<String, String>,
    overwrite: bool,
    timeout: Option<Duration>,
) -> anyhow::Result<TaskOutcome> {
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
    let initial_template = adapter
        .cli_exec_command(guard, agent_model, agent_env)
        .ok_or_else(|| anyhow!("harness declares no initial dispatch command"))?;
    // Only a multi-turn task resumes a session, and a harness may support
    // one-shot dispatch without declaring `[conversation]` at all (cline does).
    // Requiring the template up front would make those harnesses undispatchable.
    let resume_template = if plan.delivers_followups() {
        Some(
            adapter
                .cli_resume_command(guard, agent_model, agent_env)
                .ok_or_else(|| anyhow!("harness declares no native conversation resume command"))?,
        )
    } else {
        None
    };
    if overwrite && conversation_path.exists() {
        fs::remove_file(&conversation_path).with_context(|| {
            format!(
                "failed to remove stale conversation artifact {}",
                conversation_path.display()
            )
        })?;
    }

    let base_outputs = Path::new(&task.outputs_dir);
    let mut events = vec![ConversationEvent::UserMessage {
        ordinal: 0,
        round: 1,
        text: task.user_prompt.clone(),
        origin: None,
    }];
    let mut next_ordinal = 1_u32;

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
            base_outputs,
            ConversationRecord {
                status: ConversationStatus::TimedOut,
                delivered_followups: 0,
                stop_reason: None,
                stopped_before_followup: None,
                timed_out_in_round: Some(1),
                events,
            },
            None,
            plan.source(),
        );
    }
    let first_summary = parse_round(adapter, &first_outputs, &events_filename, 1)?;
    let session_id = first_summary
        .session_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| anyhow!("turn 1 transcript did not expose a native session id"))?;
    let mut preceding_assistant =
        append_summary_events(&mut events, &mut next_ordinal, 1, &first_summary)?;
    let mut final_message = first_summary
        .final_text
        .clone()
        .expect("append_summary_events requires final_text");

    let mut delivered_followups = 0_u32;
    let mut stop_reason = None;
    let mut stopped_before_followup = None;
    let mut timed_out_in_round = None;

    loop {
        let followup = delivered_followups.saturating_add(1);
        let next = plan.next_turn(delivered_followups, &preceding_assistant, &final_message)?;
        let (prompt, origin) = match next {
            NextTurn::Done => break,
            NextTurn::Stop(reason) => {
                stop_reason = Some(reason);
                stopped_before_followup = Some(followup);
                break;
            }
            NextTurn::Deliver { text, origin } => (text, origin),
        };

        let round = followup.saturating_add(1);
        events.push(ConversationEvent::UserMessage {
            ordinal: next_ordinal,
            round,
            text: prompt.clone(),
            origin,
        });
        next_ordinal = next_ordinal.saturating_add(1);
        delivered_followups = delivered_followups.saturating_add(1);

        let round_outputs = base_outputs.join(format!("turn-{round}"));
        let resume_template = resume_template
            .as_deref()
            .expect("a task that delivers follow-ups resolved a resume template above");
        let command = render_command(
            resume_template,
            eval_root,
            &task.dispatch_prompt_path,
            &round_outputs,
            Some(&session_id),
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
            && observed != session_id
        {
            bail!("turn {round} resumed session {observed:?}, expected {session_id:?}");
        }
        preceding_assistant =
            append_summary_events(&mut events, &mut next_ordinal, round, &summary)?;
        final_message = summary
            .final_text
            .clone()
            .expect("append_summary_events requires final_text");
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
        base_outputs,
        ConversationRecord {
            status,
            delivered_followups,
            stop_reason: timed_out_in_round.map_or(stop_reason, |_| None),
            stopped_before_followup: timed_out_in_round.map_or(stopped_before_followup, |_| None),
            timed_out_in_round,
            events,
        },
        Some(final_message),
        plan.source(),
    )
}

/// Validate, commit, and report one task's completion artifact. `final_message`
/// is absent when no round produced one, which is only possible for a task that
/// timed out before its first answer.
fn write_conversation(
    conversation_path: &Path,
    base_outputs: &Path,
    conversation: ConversationRecord,
    final_message: Option<String>,
    source: TurnSource,
) -> anyhow::Result<TaskOutcome> {
    let _: ConversationRecord = validate_against_schema(
        SchemaName::Conversation,
        &serde_json::to_value(&conversation)?,
        &conversation_path.to_string_lossy(),
    )?;
    write_json_atomic(conversation_path, &conversation)?;
    fs::create_dir_all(base_outputs)?;
    if let Some(final_message) = final_message {
        fs::write(
            base_outputs.join("final-message.md"),
            format!("{}\n", final_message.trim_end()),
        )?;
    }

    Ok(match conversation.status {
        ConversationStatus::Completed => TaskOutcome::Completed {
            delivered_followups: conversation.delivered_followups,
            source,
        },
        ConversationStatus::Stopped => TaskOutcome::Stopped {
            before_followup: conversation.stopped_before_followup.unwrap_or_default(),
            reason: conversation.stop_reason,
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

fn append_summary_events(
    out: &mut Vec<ConversationEvent>,
    next_ordinal: &mut u32,
    round: u32,
    summary: &TranscriptSummary,
) -> anyhow::Result<String> {
    let final_text = summary
        .final_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            anyhow!("turn {round} transcript did not contain a final assistant message")
        })?;
    let mut assistant_messages = Vec::new();
    for event in &summary.events {
        match event {
            TranscriptEvent::AssistantMessage { text, .. } => {
                assistant_messages.push(text.clone());
                out.push(ConversationEvent::AssistantMessage {
                    ordinal: *next_ordinal,
                    round,
                    text: text.clone(),
                });
            }
            TranscriptEvent::ToolInvocation {
                name, args, result, ..
            } => out.push(ConversationEvent::ToolInvocation {
                ordinal: *next_ordinal,
                round,
                name: name.clone(),
                args: args.clone(),
                result: result.clone(),
            }),
        }
        *next_ordinal = next_ordinal.saturating_add(1);
    }
    let final_already_present = assistant_messages
        .iter()
        .enumerate()
        .any(|(start, _)| assistant_messages[start..].join("\n") == final_text);
    if !final_already_present {
        assistant_messages.push(final_text.to_string());
        out.push(ConversationEvent::AssistantMessage {
            ordinal: *next_ordinal,
            round,
            text: final_text.to_string(),
        });
        *next_ordinal = next_ordinal.saturating_add(1);
    }
    Ok(assistant_messages.join("\n"))
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
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("conversation path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let temp_path = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("conversation.json")
    ));
    let mut body = serde_json::to_string_pretty(value)?;
    body.push('\n');
    fs::write(&temp_path, body)?;
    fs::rename(&temp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{append_summary_events, execute_round};
    use crate::adapters::TranscriptSummary;
    use crate::adapters::cli_command::shell_quote_arg;
    use crate::adapters::transcript::TranscriptEvent;

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
    fn joined_final_text_does_not_duplicate_assistant_text_blocks() {
        let summary = TranscriptSummary {
            tool_invocations: Vec::new(),
            events: vec![
                TranscriptEvent::AssistantMessage {
                    ordinal: 0,
                    text: "Which timezone?".into(),
                },
                TranscriptEvent::AssistantMessage {
                    ordinal: 1,
                    text: "Please include the locale.".into(),
                },
            ],
            session_id: Some("session-1".into()),
            total_tokens: None,
            duration_ms: None,
            final_text: Some("Which timezone?\nPlease include the locale.".into()),
        };
        let mut events = Vec::new();
        let mut next_ordinal = 0;

        let assistant = append_summary_events(&mut events, &mut next_ordinal, 1, &summary).unwrap();

        assert_eq!(assistant, "Which timezone?\nPlease include the locale.");
        assert_eq!(
            events.len(),
            2,
            "joined final text must not be appended again"
        );
    }
}
