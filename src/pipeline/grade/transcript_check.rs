//! Transcript-check grading.
//!
//! `tool_invocation_matches` checks regex-match the `"<name> <json-args>"`
//! rendering of tool calls. `assistant_message_matches` checks ordered
//! assistant messages in a scripted conversation. Both honor optional
//! cross-event ordering constraints.

use regex::Regex;

use crate::adapters::ToolVocabulary;
use crate::core::{
    AssertionResult, AssertionTranscriptCheck, ConversationEvent, ConversationRecord, Grader,
    MustPrecede, ToolInvocation,
};

/// Render an invocation as `"<name> <compact-json-args>"` (args omitted when
/// absent) — the text the check's `pattern` regex runs against.
fn describe_invocation(inv: &ToolInvocation) -> String {
    match &inv.args {
        Some(args) => format!(
            "{} {}",
            inv.name,
            serde_json::to_string(args).unwrap_or_default()
        ),
        None => inv.name.clone(),
    }
}

/// A failed transcript-check result with full confidence.
fn fail(id: &str, evidence: String) -> AssertionResult {
    AssertionResult {
        id: id.to_string(),
        passed: false,
        evidence,
        confidence: Some(1.0),
        grader: Some(Grader::TranscriptCheck),
    }
}

/// Grade a `transcript_check` assertion against a run's tool invocations,
/// covering the empty-invocations, unsupported-kind, missing-pattern,
/// invalid-regex, match, and no-match branches.
pub fn grade_transcript_check(
    assertion: &AssertionTranscriptCheck,
    invocations: &[ToolInvocation],
) -> AssertionResult {
    grade_transcript_check_with_context(assertion, invocations, None, &ToolVocabulary::default())
}

/// Grade with the ordered conversation and harness write vocabulary available.
/// The legacy wrapper above remains for one-shot callers and tests.
pub fn grade_transcript_check_with_context(
    assertion: &AssertionTranscriptCheck,
    invocations: &[ToolInvocation],
    conversation: Option<&ConversationRecord>,
    vocabulary: &ToolVocabulary,
) -> AssertionResult {
    if !matches!(
        assertion.check.as_str(),
        "tool_invocation_matches" | "assistant_message_matches"
    ) {
        return fail(
            &assertion.id,
            format!("unsupported transcript_check kind: '{}'", assertion.check),
        );
    }

    if assertion.check == "tool_invocation_matches" && invocations.is_empty() {
        return fail(
            &assertion.id,
            "tool_invocations is empty — run record was not filled by a transcript adapter. \
             Run `eval-magic fill-transcripts` for Claude Code, or `eval-magic fill-transcripts \
             --harness codex` when outputs/codex-events.jsonl is present; otherwise rely on \
             `llm_judge` assertions for harnesses without an adapter."
                .to_string(),
        );
    }

    if assertion.check == "assistant_message_matches" && conversation.is_none() {
        return fail(
            &assertion.id,
            "assistant_message_matches requires a scripted conversation artifact; this run is \
             one-shot or was not ingested from conversation.json"
                .to_string(),
        );
    }

    let Some(pattern) = assertion.pattern.as_deref() else {
        return fail(
            &assertion.id,
            format!(
                "transcript_check '{}' requires a `pattern` field",
                assertion.check
            ),
        );
    };

    let re = match Regex::new(pattern) {
        Ok(re) => re,
        Err(err) => {
            return fail(
                &assertion.id,
                format!("invalid regex in pattern '{pattern}': {err}"),
            );
        }
    };

    let limit = ordering_limit(
        assertion.must_precede,
        conversation,
        invocations,
        vocabulary,
    );
    let order_name = ordering_name(assertion.must_precede);
    let mut regex_matches = 0_usize;

    if assertion.check == "tool_invocation_matches" {
        for inv in invocations {
            let target = describe_invocation(inv);
            if re.is_match(&target) {
                regex_matches += 1;
                if limit.is_none_or(|ordinal| inv.ordinal < ordinal) {
                    return passed(&assertion.id, inv.ordinal, &target);
                }
            }
        }
    } else if let Some(conversation) = conversation {
        for event in &conversation.events {
            let ConversationEvent::AssistantMessage { ordinal, text, .. } = event else {
                continue;
            };
            if re.is_match(text) {
                regex_matches += 1;
                if limit.is_none_or(|limit| *ordinal < limit) {
                    return passed(&assertion.id, *ordinal, text);
                }
            }
        }
    }

    if regex_matches > 0 && limit.is_some() {
        return fail(
            &assertion.id,
            format!(
                "{regex_matches} match(es) for /{pattern}/ occurred, but none before {order_name}"
            ),
        );
    }

    let candidate_name = if assertion.check == "tool_invocation_matches" {
        format!("{} invocation(s)", invocations.len())
    } else {
        format!(
            "{} assistant message(s)",
            conversation
                .map(|conversation| {
                    conversation
                        .events
                        .iter()
                        .filter(|event| matches!(event, ConversationEvent::AssistantMessage { .. }))
                        .count()
                })
                .unwrap_or_default()
        )
    };
    fail(
        &assertion.id,
        format!("no candidate matched /{pattern}/ across {candidate_name}"),
    )
}

fn passed(id: &str, ordinal: u32, target: &str) -> AssertionResult {
    let snippet: String = target.chars().take(200).collect();
    AssertionResult {
        id: id.to_string(),
        passed: true,
        evidence: format!("matched ordinal {ordinal}: {snippet}"),
        confidence: Some(1.0),
        grader: Some(Grader::TranscriptCheck),
    }
}

fn ordering_limit(
    constraint: Option<MustPrecede>,
    conversation: Option<&ConversationRecord>,
    invocations: &[ToolInvocation],
    vocabulary: &ToolVocabulary,
) -> Option<u32> {
    match constraint.unwrap_or(MustPrecede::Any) {
        MustPrecede::Any => None,
        MustPrecede::CompletionClaim => conversation.and_then(|conversation| {
            conversation
                .events
                .iter()
                .rev()
                .find_map(|event| match event {
                    ConversationEvent::AssistantMessage { ordinal, .. } => Some(*ordinal),
                    _ => None,
                })
        }),
        MustPrecede::FirstWrite => conversation
            .and_then(|conversation| {
                conversation.events.iter().find_map(|event| match event {
                    ConversationEvent::ToolInvocation { ordinal, name, .. }
                        if is_write(name, vocabulary) =>
                    {
                        Some(*ordinal)
                    }
                    _ => None,
                })
            })
            .or_else(|| {
                invocations
                    .iter()
                    .find(|invocation| is_write(&invocation.name, vocabulary))
                    .map(|invocation| invocation.ordinal)
            }),
    }
}

fn is_write(name: &str, vocabulary: &ToolVocabulary) -> bool {
    vocabulary.write_tools.iter().any(|tool| tool == name)
        || vocabulary.patch_tools.iter().any(|tool| tool == name)
}

fn ordering_name(constraint: Option<MustPrecede>) -> &'static str {
    match constraint.unwrap_or(MustPrecede::Any) {
        MustPrecede::CompletionClaim => "the final completion claim",
        MustPrecede::FirstWrite => "the first write",
        MustPrecede::Any => "the end of the run",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::ToolVocabulary;
    use crate::core::{ConversationEvent, ConversationRecord, ConversationStatus, MustPrecede};
    use serde_json::json;

    fn check(pattern: Option<&str>) -> AssertionTranscriptCheck {
        AssertionTranscriptCheck {
            id: "t1".to_string(),
            check: "tool_invocation_matches".to_string(),
            pattern: pattern.map(str::to_string),
            must_precede: None,
        }
    }

    fn inv(name: &str, args: serde_json::Value, ordinal: u32) -> ToolInvocation {
        ToolInvocation {
            name: name.to_string(),
            args: Some(args),
            result: None,
            ordinal,
        }
    }

    #[test]
    fn empty_invocations_fail_with_guidance() {
        let r = grade_transcript_check(&check(Some("Bash")), &[]);
        assert!(!r.passed);
        assert!(r.evidence.contains("tool_invocations is empty"));
        assert_eq!(r.grader, Some(Grader::TranscriptCheck));
    }

    #[test]
    fn missing_pattern_fails() {
        let r = grade_transcript_check(&check(None), &[inv("Bash", json!({"command": "ls"}), 0)]);
        assert!(!r.passed);
        assert!(r.evidence.contains("requires a `pattern`"));
    }

    #[test]
    fn unsupported_kind_fails() {
        let mut c = check(Some("x"));
        c.check = "something_else".to_string();
        let r = grade_transcript_check(&c, &[inv("Bash", json!({}), 0)]);
        assert!(!r.passed);
        assert!(r.evidence.contains("unsupported transcript_check kind"));
    }

    #[test]
    fn matching_pattern_passes_with_ordinal() {
        let invs = [
            inv("Read", json!({"file_path": "/x"}), 0),
            inv("Bash", json!({"command": "bun test"}), 1),
        ];
        let r = grade_transcript_check(&check(Some("bun test")), &invs);
        assert!(r.passed);
        assert!(r.evidence.contains("matched ordinal 1"));
    }

    #[test]
    fn no_match_fails_with_count() {
        let invs = [inv("Read", json!({"file_path": "/x"}), 0)];
        let r = grade_transcript_check(&check(Some("npm install")), &invs);
        assert!(!r.passed);
        assert!(r.evidence.contains("across 1 invocation(s)"));
    }

    #[test]
    fn invalid_regex_fails() {
        let invs = [inv("Bash", json!({"command": "ls"}), 0)];
        let r = grade_transcript_check(&check(Some("(unclosed")), &invs);
        assert!(!r.passed);
        assert!(r.evidence.contains("invalid regex"));
    }

    fn conversation() -> ConversationRecord {
        ConversationRecord {
            status: ConversationStatus::Completed,
            delivered_followups: 1,
            stop_reason: None,
            stopped_before_followup: None,
            timed_out_in_round: None,
            events: vec![
                ConversationEvent::UserMessage {
                    ordinal: 0,
                    round: 1,
                    text: "Fix it".into(),
                },
                ConversationEvent::AssistantMessage {
                    ordinal: 1,
                    round: 1,
                    text: "Which timezone?".into(),
                },
                ConversationEvent::UserMessage {
                    ordinal: 2,
                    round: 2,
                    text: "US timezones".into(),
                },
                ConversationEvent::ToolInvocation {
                    ordinal: 3,
                    round: 2,
                    name: "Write".into(),
                    args: Some(json!({"file_path": "date.rs"})),
                    result: None,
                },
                ConversationEvent::AssistantMessage {
                    ordinal: 4,
                    round: 2,
                    text: "Done.".into(),
                },
            ],
        }
    }

    #[test]
    fn assistant_message_matches_across_conversation_rounds() {
        let assertion = AssertionTranscriptCheck {
            id: "asked".into(),
            check: "assistant_message_matches".into(),
            pattern: Some("(?i)time ?zone".into()),
            must_precede: Some(MustPrecede::FirstWrite),
        };
        let result = grade_transcript_check_with_context(
            &assertion,
            &[],
            Some(&conversation()),
            &ToolVocabulary {
                write_tools: vec!["Write".into()],
                ..Default::default()
            },
        );
        assert!(result.passed, "{}", result.evidence);
        assert!(result.evidence.contains("ordinal 1"));
    }

    #[test]
    fn assistant_message_after_first_write_fails_ordering_constraint() {
        let assertion = AssertionTranscriptCheck {
            id: "late".into(),
            check: "assistant_message_matches".into(),
            pattern: Some("Done".into()),
            must_precede: Some(MustPrecede::FirstWrite),
        };
        let result = grade_transcript_check_with_context(
            &assertion,
            &[],
            Some(&conversation()),
            &ToolVocabulary {
                write_tools: vec!["Write".into()],
                ..Default::default()
            },
        );
        assert!(!result.passed);
        assert!(result.evidence.contains("first write"));
    }
}
