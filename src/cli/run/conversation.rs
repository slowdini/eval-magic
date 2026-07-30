//! Runner-owned execution of one scripted multi-turn dispatch task.
//!
//! The run workspace persists the resolved harness descriptor alongside each
//! task. This driver uses that frozen descriptor to start one native session,
//! gate and deliver each canned user follow-up, and write an ordered
//! `conversation.json` completion artifact for ingest.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, anyhow, bail};
use regex::Regex;
use serde::Deserialize;

use crate::adapters::cli_command::shell_quote_arg;
use crate::adapters::descriptor::{finalize_descriptor, subst};
use crate::adapters::descriptor_adapter::DescriptorAdapter;
use crate::adapters::harness::HarnessAdapter;
use crate::adapters::transcript::{TranscriptEvent, TranscriptSummary};
use crate::core::{
    ConversationEvent, ConversationRecord, ConversationStatus, ConversationStopReason, DeliverWhen,
    ScriptedTurn, validate_agent_environment_entry,
};
use crate::validation::{SchemaName, validate_against_schema};

use super::dispatch::DispatchTask;

#[derive(Debug, Deserialize)]
struct DispatchEnvelope {
    #[serde(default)]
    guard: bool,
    #[serde(default)]
    agent_model: Option<String>,
    #[serde(default)]
    agent_env: BTreeMap<String, String>,
    harness_descriptor: serde_json::Value,
    tasks: Vec<DispatchTask>,
}

/// Execute task `task_index` from a runner-generated dispatch plan.
pub fn command_dispatch_task(
    dispatch_path: &Path,
    task_index: usize,
    overwrite: bool,
) -> anyhow::Result<()> {
    let raw = fs::read_to_string(dispatch_path)
        .with_context(|| format!("failed to read {}", dispatch_path.display()))?;
    let envelope: DispatchEnvelope = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", dispatch_path.display()))?;
    for (name, value) in &envelope.agent_env {
        validate_agent_environment_entry(name, value).map_err(|message| {
            anyhow!(
                "invalid agent_env in {}: {message}",
                dispatch_path.display()
            )
        })?;
    }
    let descriptor = finalize_descriptor(
        &envelope.harness_descriptor,
        &format!("{}#harness_descriptor", dispatch_path.display()),
    )?;
    let adapter = DescriptorAdapter::from_descriptor(descriptor);
    let task = envelope.tasks.get(task_index).ok_or_else(|| {
        anyhow!(
            "--task-index {task_index} is out of range for {} task(s)",
            envelope.tasks.len()
        )
    })?;
    run_task(
        &adapter,
        task,
        envelope.guard,
        envelope.agent_model.as_deref(),
        &envelope.agent_env,
        overwrite,
    )
}

fn run_task(
    adapter: &DescriptorAdapter,
    task: &DispatchTask,
    guard: bool,
    agent_model: Option<&str>,
    agent_env: &BTreeMap<String, String>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let turns = task
        .turns
        .as_deref()
        .filter(|turns| !turns.is_empty())
        .ok_or_else(|| anyhow!("selected task does not declare scripted follow-up turns"))?;
    let conversation_path = task
        .conversation_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("multi-turn task is missing conversation_path"))?;
    if conversation_path.exists() && !overwrite {
        bail!(
            "{} already exists; pass --overwrite to rerun this conversation",
            conversation_path.display()
        );
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
    let resume_template = adapter
        .cli_resume_command(guard, agent_model, agent_env)
        .ok_or_else(|| anyhow!("harness declares no native conversation resume command"))?;
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
    execute_round(
        &initial_command,
        Path::new(eval_root),
        &first_outputs,
        agent_env,
        1,
    )?;
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

    for (index, turn) in turns.iter().enumerate() {
        let followup = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if let Some(reason) = unmet_gate(turn, &preceding_assistant)? {
            stop_reason = Some(reason);
            stopped_before_followup = Some(followup);
            break;
        }

        let round = followup.saturating_add(1);
        events.push(ConversationEvent::UserMessage {
            ordinal: next_ordinal,
            round,
            text: turn.prompt.clone(),
        });
        next_ordinal = next_ordinal.saturating_add(1);
        delivered_followups = delivered_followups.saturating_add(1);

        let round_outputs = base_outputs.join(format!("turn-{round}"));
        let command = render_command(
            &resume_template,
            eval_root,
            &task.dispatch_prompt_path,
            &round_outputs,
            Some(&session_id),
            Some(&turn.prompt),
            round,
        );
        execute_round(
            &command,
            Path::new(eval_root),
            &round_outputs,
            agent_env,
            round,
        )?;
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

    let conversation = ConversationRecord {
        status: if stop_reason.is_some() {
            ConversationStatus::Stopped
        } else {
            ConversationStatus::Completed
        },
        delivered_followups,
        stop_reason,
        stopped_before_followup,
        events,
    };
    let _: ConversationRecord = validate_against_schema(
        SchemaName::Conversation,
        &serde_json::to_value(&conversation)?,
        &conversation_path.to_string_lossy(),
    )?;
    write_json_atomic(&conversation_path, &conversation)?;
    fs::create_dir_all(base_outputs)?;
    fs::write(
        base_outputs.join("final-message.md"),
        format!("{}\n", final_message.trim_end()),
    )?;

    match conversation.status {
        ConversationStatus::Completed => println!(
            "Completed {} scripted follow-up turn(s): {}",
            delivered_followups,
            conversation_path.display()
        ),
        ConversationStatus::Stopped => println!(
            "Stopped before scripted follow-up {} ({:?}): {}",
            conversation.stopped_before_followup.unwrap_or_default(),
            conversation
                .stop_reason
                .expect("stopped conversations have a reason"),
            conversation_path.display()
        ),
    }
    Ok(())
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

fn unmet_gate(
    turn: &ScriptedTurn,
    preceding_assistant: &str,
) -> anyhow::Result<Option<ConversationStopReason>> {
    if turn.deliver_when == DeliverWhen::Always {
        return Ok(None);
    }
    if !preceding_assistant.contains('?') {
        return Ok(Some(ConversationStopReason::AgentDidNotAsk));
    }
    if let Some(pattern) = &turn.agent_response_matches {
        let regex = Regex::new(pattern)
            .with_context(|| format!("invalid agent_response_matches regex {pattern:?}"))?;
        if !regex.is_match(preceding_assistant) {
            return Ok(Some(ConversationStopReason::AgentResponseMismatch));
        }
    }
    Ok(None)
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

fn execute_round(
    command: &str,
    eval_root: &Path,
    outputs_dir: &Path,
    agent_env: &BTreeMap<String, String>,
    round: u32,
) -> anyhow::Result<()> {
    fs::create_dir_all(outputs_dir)
        .with_context(|| format!("failed to create turn {round} outputs"))?;
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(eval_root)
        .envs(agent_env)
        .status()
        .with_context(|| format!("failed to start harness command for turn {round}"))?;
    if !status.success() {
        bail!("harness command for turn {round} exited with {status}");
    }
    Ok(())
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

    use super::{append_summary_events, execute_round, unmet_gate};
    use crate::adapters::TranscriptSummary;
    use crate::adapters::cli_command::shell_quote_arg;
    use crate::adapters::transcript::TranscriptEvent;
    use crate::core::{ConversationStopReason, DeliverWhen, ScriptedTurn};

    fn conditional(pattern: Option<&str>) -> ScriptedTurn {
        ScriptedTurn {
            prompt: "follow up".into(),
            deliver_when: DeliverWhen::AgentAsks,
            agent_response_matches: pattern.map(str::to_string),
        }
    }

    #[test]
    fn agent_asks_requires_a_question_mark() {
        assert_eq!(
            unmet_gate(&conditional(None), "Please provide the timezone.").unwrap(),
            Some(ConversationStopReason::AgentDidNotAsk)
        );
        assert_eq!(
            unmet_gate(&conditional(None), "Which timezone?").unwrap(),
            None
        );
    }

    #[test]
    fn response_pattern_is_an_additional_compatibility_gate() {
        assert_eq!(
            unmet_gate(&conditional(Some("(?i)time ?zone")), "Which locale?").unwrap(),
            Some(ConversationStopReason::AgentResponseMismatch)
        );
        assert_eq!(
            unmet_gate(
                &conditional(Some("(?i)time ?zone")),
                "Which timezone should I use?"
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn execute_round_creates_the_round_output_directory_before_shell_redirection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outputs = tmp.path().join("outputs").join("turn-1");
        let events = outputs.join("events.jsonl");
        let command = format!(
            "printf '%s\\n' '{{\"type\":\"done\"}}' > {}",
            shell_quote_arg(&events.to_string_lossy())
        );

        execute_round(&command, tmp.path(), &outputs, &BTreeMap::new(), 1).unwrap();

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
