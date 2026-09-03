//! Transcript-check grading.
//!
//! `tool_invocation_matches` checks regex-match the `"<name> <json-args>"`
//! rendering of tool calls. `assistant_message_matches` checks ordered
//! assistant messages in a scripted conversation. Both honor optional
//! cross-event ordering constraints.
//!
//! Tool names are matched *portably*. A harness picks its own spelling for the
//! same behavior — Claude Code's `Bash` is Codex's `command_execution` — so an
//! authored pattern is tried against the native rendering first and then
//! against one rendering per portable alias for the invocation's role. The role
//! comes from the run's own descriptor (roles are disjoint there) and the alias
//! spellings from the registry-wide union, so no harness is named here and a
//! BYOH descriptor opts in through its `[tools]` vocabulary alone.

use regex::Regex;

use crate::adapters::{EMPTY_TOOL_VOCABULARY, ToolRole, ToolVocabulary};
use crate::core::{
    AssertionResult, AssertionTranscriptCheck, ConversationEvent, ConversationRecord, Grader,
    MustPrecede, ToolInvocation,
};

/// How a run's tool names are read: the run's own descriptor decides which role
/// a native name plays; the registry-wide union supplies the portable alias
/// spellings for that role.
pub struct ToolNaming<'a> {
    active: &'a ToolVocabulary,
    aliases: &'a ToolVocabulary,
}

impl<'a> ToolNaming<'a> {
    pub fn new(active: &'a ToolVocabulary, aliases: &'a ToolVocabulary) -> Self {
        Self { active, aliases }
    }

    /// Native-only matching: roles still classify ordering, but no alias is
    /// ever substituted — the shape a caller with no harness registry gets.
    pub fn without_aliases(active: &'a ToolVocabulary) -> Self {
        Self {
            active,
            aliases: &EMPTY_TOOL_VOCABULARY,
        }
    }

    /// The role the run's own descriptor declares for `name`.
    fn role_of(&self, name: &str) -> Option<ToolRole> {
        self.active.role_of(name)
    }

    /// Every portable spelling of `role`, from the registry-wide union.
    fn portable_names(&self, role: ToolRole) -> &'a [String] {
        self.aliases.names_in(role)
    }
}

/// Which rendering of an invocation the pattern matched.
enum InvocationMatch<'a> {
    Native,
    Alias { role: ToolRole, alias: &'a str },
}

/// Render an invocation as `"<name> <compact-json-args>"` (args omitted when
/// absent) — the text the check's `pattern` regex runs against.
fn describe_invocation(inv: &ToolInvocation) -> String {
    describe_with_name(&inv.name, inv)
}

/// The same rendering under a substituted tool name; arguments are preserved
/// verbatim, so a pattern over arguments behaves identically either way.
fn describe_with_name(name: &str, inv: &ToolInvocation) -> String {
    match &inv.args {
        Some(args) => format!("{name} {}", serde_json::to_string(args).unwrap_or_default()),
        None => name.to_string(),
    }
}

/// Match `re` against one invocation: the native rendering first, then one
/// alias variant per portable spelling of its role. The first hit wins, so the
/// outcome is deterministic.
fn match_invocation<'a>(
    re: &Regex,
    inv: &ToolInvocation,
    naming: &ToolNaming<'a>,
) -> Option<InvocationMatch<'a>> {
    if re.is_match(&describe_invocation(inv)) {
        return Some(InvocationMatch::Native);
    }
    let role = naming.role_of(&inv.name)?;
    let alias = naming
        .portable_names(role)
        .iter()
        .map(String::as_str)
        // The native name was already tried above.
        .find(|alias| *alias != inv.name && re.is_match(&describe_with_name(alias, inv)))?;
    Some(InvocationMatch::Alias { role, alias })
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
    grade_transcript_check_with_context(
        assertion,
        invocations,
        None,
        &ToolNaming::without_aliases(&EMPTY_TOOL_VOCABULARY),
    )
}

/// Grade with the ordered conversation and the run's tool naming available.
/// The legacy wrapper above remains for one-shot callers and tests.
pub fn grade_transcript_check_with_context(
    assertion: &AssertionTranscriptCheck,
    invocations: &[ToolInvocation],
    conversation: Option<&ConversationRecord>,
    naming: &ToolNaming<'_>,
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
            "tool_invocations is empty — the task transcript contained no recorded tool call. \
             Re-dispatch the task if its transcript is missing; otherwise verify the agent \
             actually invoked the expected tool."
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

    let limit = ordering_limit(assertion.must_precede, conversation, invocations, naming);
    let order_name = ordering_name(assertion.must_precede);
    let mut regex_matches = 0_usize;

    if assertion.check == "tool_invocation_matches" {
        for inv in invocations {
            let Some(matched) = match_invocation(&re, inv, naming) else {
                continue;
            };
            regex_matches += 1;
            if limit.is_none_or(|ordinal| inv.ordinal < ordinal) {
                let native = describe_invocation(inv);
                return match matched {
                    InvocationMatch::Native => passed(&assertion.id, inv.ordinal, &native),
                    InvocationMatch::Alias { role, alias } => {
                        passed_via_alias(&assertion.id, inv.ordinal, role, alias, &native)
                    }
                };
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
    let expanded = if assertion.check == "tool_invocation_matches" {
        expanded_roles(invocations, naming)
    } else {
        String::new()
    };
    fail(
        &assertion.id,
        format!("no candidate matched /{pattern}/ across {candidate_name}{expanded}"),
    )
}

/// The roles whose aliases were tried, rendered as an evidence suffix. Empty
/// when nothing could be expanded, which keeps a native-only run's message
/// exactly as it read before alias matching existed.
fn expanded_roles(invocations: &[ToolInvocation], naming: &ToolNaming<'_>) -> String {
    let names: Vec<&str> = ToolRole::ALL
        .into_iter()
        .filter(|role| {
            invocations.iter().any(|inv| {
                naming.role_of(&inv.name) == Some(*role)
                    && naming
                        .portable_names(*role)
                        .iter()
                        .any(|alias| alias != &inv.name)
            })
        })
        .map(ToolRole::as_str)
        .collect();
    if names.is_empty() {
        String::new()
    } else {
        format!(" (native names plus {} role aliases)", names.join("/"))
    }
}

fn passed(id: &str, ordinal: u32, target: &str) -> AssertionResult {
    result_for(id, ordinal, target, None)
}

/// A pass a portable alias supplied. The evidence reports the *native*
/// invocation — what the harness actually recorded — and names the alias and
/// role that matched it, so a reader can tell the two apart.
fn passed_via_alias(
    id: &str,
    ordinal: u32,
    role: ToolRole,
    alias: &str,
    native: &str,
) -> AssertionResult {
    result_for(id, ordinal, native, Some((role, alias)))
}

fn result_for(
    id: &str,
    ordinal: u32,
    target: &str,
    via: Option<(ToolRole, &str)>,
) -> AssertionResult {
    let snippet: String = target.chars().take(200).collect();
    let via = via
        .map(|(role, alias)| format!(" via {} alias '{alias}'", role.as_str()))
        .unwrap_or_default();
    AssertionResult {
        id: id.to_string(),
        passed: true,
        evidence: format!("matched ordinal {ordinal}{via}: {snippet}"),
        confidence: Some(1.0),
        grader: Some(Grader::TranscriptCheck),
    }
}

fn ordering_limit(
    constraint: Option<MustPrecede>,
    conversation: Option<&ConversationRecord>,
    invocations: &[ToolInvocation],
    naming: &ToolNaming<'_>,
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
                        if is_write(name, naming) =>
                    {
                        Some(*ordinal)
                    }
                    _ => None,
                })
            })
            .or_else(|| {
                invocations
                    .iter()
                    .find(|invocation| is_write(&invocation.name, naming))
                    .map(|invocation| invocation.ordinal)
            }),
    }
}

/// Ordering classifies against the run's own vocabulary only: the union could
/// call another harness's name a write when this harness never emits it.
fn is_write(name: &str, naming: &ToolNaming<'_>) -> bool {
    matches!(
        naming.active.role_of(name),
        Some(ToolRole::Write | ToolRole::Patch)
    )
}

fn ordering_name(constraint: Option<MustPrecede>) -> &'static str {
    match constraint.unwrap_or(MustPrecede::Any) {
        MustPrecede::CompletionClaim => "the final completion claim",
        MustPrecede::FirstWrite => "the first write",
        MustPrecede::Any => "the end of the run",
    }
}

#[cfg(test)]
mod alias_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::ToolVocabulary;
    use crate::core::{ConversationEvent, ConversationRecord, ConversationStatus, MustPrecede};
    use serde_json::json;

    pub(super) fn check(pattern: Option<&str>) -> AssertionTranscriptCheck {
        AssertionTranscriptCheck {
            id: "t1".to_string(),
            check: "tool_invocation_matches".to_string(),
            pattern: pattern.map(str::to_string),
            must_precede: None,
        }
    }

    pub(super) fn inv(name: &str, args: serde_json::Value, ordinal: u32) -> ToolInvocation {
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
                    origin: None,
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
                    origin: None,
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
            responder_outcome: None,
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
        let active = ToolVocabulary {
            write_tools: vec!["Write".into()],
            ..Default::default()
        };
        let result = grade_transcript_check_with_context(
            &assertion,
            &[],
            Some(&conversation()),
            &ToolNaming::without_aliases(&active),
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
        let active = ToolVocabulary {
            write_tools: vec!["Write".into()],
            ..Default::default()
        };
        let result = grade_transcript_check_with_context(
            &assertion,
            &[],
            Some(&conversation()),
            &ToolNaming::without_aliases(&active),
        );
        assert!(!result.passed);
        assert!(result.evidence.contains("first write"));
    }
}
