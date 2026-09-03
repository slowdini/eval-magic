use super::*;
use crate::adapters::TokenUsageAggregation;
use crate::adapters::transcript::TranscriptEvent;
use crate::core::{ConversationEvent, ConversationRecord, ToolInvocation};

pub(super) struct TaskEvidence {
    pub summary: Option<TranscriptSummary>,
    pub conversation: ConversationRecord,
    pub transcripts_complete: bool,
}

pub(super) fn for_task(task: &DispatchTask) -> Result<Option<ConversationRecord>, PipelineError> {
    let Some(path) = task.conversation_path.as_deref() else {
        return Ok(None);
    };
    let path = Path::new(path);
    if !path.exists() {
        return Ok(None);
    }
    let value = serde_json::from_str(&fs::read_to_string(path)?)?;
    Ok(Some(validate_against_schema(
        SchemaName::Conversation,
        &value,
        &path.to_string_lossy(),
    )?))
}

/// Combine runner-owned user-turn metadata with transcript-owned assistant and
/// tool events. Historical assistant/tool events in the completion artifact are
/// deliberately ignored so one source owns their bytes and ordering.
pub(super) fn evidence_for_task(
    harness: Harness,
    task: &DispatchTask,
    completion: &ConversationRecord,
) -> TaskEvidence {
    let rounds = completion.delivered_followups.saturating_add(1);
    let Some(filename) = adapter_for(harness).cli_events_filename() else {
        let mut conversation = completion.clone();
        conversation
            .events
            .retain(|event| matches!(event, ConversationEvent::UserMessage { .. }));
        renumber(&mut conversation.events);
        return TaskEvidence {
            summary: None,
            conversation,
            transcripts_complete: false,
        };
    };

    let mut conversation = completion.clone();
    conversation.events.clear();
    let mut transcript_events = Vec::new();
    let mut summaries = Vec::new();
    let mut complete = true;

    for round in 1..=rounds {
        for event in completion.events.iter().filter(|event| {
            matches!(event, ConversationEvent::UserMessage { round: event_round, .. } if *event_round == round)
        }) {
            let ConversationEvent::UserMessage {
                text,
                origin,
                mode,
                ..
            } = event
            else {
                unreachable!("filter retains only user messages");
            };
            conversation.events.push(ConversationEvent::UserMessage {
                ordinal: conversation.events.len() as u32,
                round,
                text: text.clone(),
                origin: origin.clone(),
                mode: *mode,
            });
        }

        let path = Path::new(&task.outputs_dir)
            .join(format!("turn-{round}"))
            .join(&filename);
        if !path.exists() {
            complete = false;
            continue;
        }
        let summary = match adapter_for(harness).parse_cli_events_full(&path) {
            Ok(summary) => summary,
            Err(_) => {
                complete = false;
                continue;
            }
        };

        let mut final_text_present = false;
        for event in &summary.events {
            let ordinal = conversation.events.len() as u32;
            match event {
                TranscriptEvent::AssistantMessage { text, .. } => {
                    final_text_present |= summary.final_text.as_deref() == Some(text);
                    conversation
                        .events
                        .push(ConversationEvent::AssistantMessage {
                            ordinal,
                            round,
                            text: text.clone(),
                        });
                    transcript_events.push(TranscriptEvent::AssistantMessage {
                        ordinal,
                        text: text.clone(),
                    });
                }
                TranscriptEvent::ToolInvocation {
                    name, args, result, ..
                } => {
                    conversation.events.push(ConversationEvent::ToolInvocation {
                        ordinal,
                        round,
                        name: name.clone(),
                        args: args.clone(),
                        result: result.clone(),
                    });
                    transcript_events.push(TranscriptEvent::ToolInvocation {
                        ordinal,
                        name: name.clone(),
                        args: args.clone(),
                        result: result.clone(),
                    });
                }
            }
        }
        if let Some(text) = &summary.final_text
            && !final_text_present
        {
            let ordinal = conversation.events.len() as u32;
            conversation
                .events
                .push(ConversationEvent::AssistantMessage {
                    ordinal,
                    round,
                    text: text.clone(),
                });
            transcript_events.push(TranscriptEvent::AssistantMessage {
                ordinal,
                text: text.clone(),
            });
        }
        summaries.push(summary);
    }

    let summary = (!summaries.is_empty()).then(|| {
        let total_tokens = match adapter_for(harness).conversation_token_usage_aggregation() {
            TokenUsageAggregation::Sum => {
                sum_some(summaries.iter().map(|summary| summary.total_tokens))
            }
            TokenUsageAggregation::Last => {
                summaries.last().and_then(|summary| summary.total_tokens)
            }
        };
        let duration_ms = sum_some(summaries.iter().map(|summary| summary.duration_ms));
        let final_text = summaries
            .iter()
            .rev()
            .find_map(|summary| summary.final_text.clone());
        TranscriptSummary {
            tool_invocations: tool_invocations(&conversation),
            events: transcript_events,
            session_id: None,
            total_tokens,
            duration_ms,
            final_text,
        }
    });

    TaskEvidence {
        summary,
        conversation,
        transcripts_complete: complete,
    }
}

fn renumber(events: &mut [ConversationEvent]) {
    for (ordinal, event) in events.iter_mut().enumerate() {
        match event {
            ConversationEvent::UserMessage { ordinal: value, .. }
            | ConversationEvent::AssistantMessage { ordinal: value, .. }
            | ConversationEvent::ToolInvocation { ordinal: value, .. } => *value = ordinal as u32,
        }
    }
}

fn sum_some(values: impl Iterator<Item = Option<i64>>) -> Option<i64> {
    values.fold(None, |sum, value| match (sum, value) {
        (None, None) => None,
        (Some(sum), None) => Some(sum),
        (None, Some(value)) => Some(value),
        (Some(sum), Some(value)) => Some(sum.saturating_add(value)),
    })
}

pub(super) fn tool_invocations(conversation: &ConversationRecord) -> Vec<ToolInvocation> {
    conversation
        .events
        .iter()
        .filter_map(|event| match event {
            ConversationEvent::ToolInvocation {
                ordinal,
                name,
                args,
                result,
                ..
            } => Some(ToolInvocation {
                name: name.clone(),
                args: args.clone(),
                ordinal: *ordinal,
                result: result.clone(),
            }),
            _ => None,
        })
        .collect()
}
