//! Transcript-check grading.
//!
//! A
//! `transcript_check` assertion of kind `tool_invocation_matches` passes when its
//! `pattern` regex matches the `"<name> <json-args>"` rendering of any tool
//! invocation in the run.

use regex::Regex;

use crate::core::{AssertionResult, AssertionTranscriptCheck, Grader, ToolInvocation};

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
    if invocations.is_empty() {
        return fail(
            &assertion.id,
            "tool_invocations is empty — run record was not filled by a transcript adapter. \
             Run `eval-magic fill-transcripts` for Claude Code, or `eval-magic fill-transcripts \
             --harness codex` when outputs/codex-events.jsonl is present; otherwise rely on \
             `llm_judge` assertions for harnesses without an adapter."
                .to_string(),
        );
    }

    if assertion.check != "tool_invocation_matches" {
        return fail(
            &assertion.id,
            format!("unsupported transcript_check kind: '{}'", assertion.check),
        );
    }

    let Some(pattern) = assertion.pattern.as_deref() else {
        return fail(
            &assertion.id,
            "transcript_check 'tool_invocation_matches' requires a `pattern` field".to_string(),
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

    for inv in invocations {
        let target = describe_invocation(inv);
        if re.is_match(&target) {
            let snippet: String = target.chars().take(200).collect();
            return AssertionResult {
                id: assertion.id.clone(),
                passed: true,
                evidence: format!("matched ordinal {}: {snippet}", inv.ordinal),
                confidence: Some(1.0),
                grader: Some(Grader::TranscriptCheck),
            };
        }
    }

    fail(
        &assertion.id,
        format!(
            "no tool invocation matched /{pattern}/ across {} invocation(s)",
            invocations.len()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
