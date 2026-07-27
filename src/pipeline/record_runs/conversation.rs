use super::*;
use crate::adapters::TokenUsageAggregation;
use crate::core::{ConversationRecord, ToolInvocation};

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

pub(super) fn summary_for_task(
    harness: Harness,
    task: &DispatchTask,
    conversation: &ConversationRecord,
) -> (Option<TranscriptSummary>, bool) {
    let rounds = conversation.delivered_followups.saturating_add(1);
    let Some(filename) = adapter_for(harness).cli_events_filename() else {
        return (None, false);
    };
    let mut summaries = Vec::new();
    let mut complete = true;
    for round in 1..=rounds {
        let path = Path::new(&task.outputs_dir)
            .join(format!("turn-{round}"))
            .join(&filename);
        if !path.exists() {
            complete = false;
            continue;
        }
        match adapter_for(harness).parse_cli_events_full(&path) {
            Ok(summary) => summaries.push(summary),
            Err(_) => complete = false,
        }
    }
    if summaries.is_empty() {
        return (None, complete);
    }
    let total_tokens = match adapter_for(harness).conversation_token_usage_aggregation() {
        TokenUsageAggregation::Sum => {
            sum_some(summaries.iter().map(|summary| summary.total_tokens))
        }
        TokenUsageAggregation::Last => summaries.last().and_then(|summary| summary.total_tokens),
    };
    let duration_ms = sum_some(summaries.iter().map(|summary| summary.duration_ms));
    (
        Some(TranscriptSummary {
            tool_invocations: tool_invocations(conversation),
            events: Vec::new(),
            session_id: None,
            total_tokens,
            duration_ms,
            final_text: None,
        }),
        complete,
    )
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
