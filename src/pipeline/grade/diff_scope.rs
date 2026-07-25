//! Deterministic grading for final-environment diff-scope assertions.

use crate::core::{AssertionDiffScope, AssertionResult, Grader};
use crate::pipeline::DiffScopeMetrics;

/// Compare the measured final-environment diff with every configured limit.
pub fn grade_diff_scope(
    assertion: &AssertionDiffScope,
    metrics: DiffScopeMetrics,
) -> AssertionResult {
    let files_passed = assertion
        .max_files_touched
        .is_none_or(|limit| metrics.files_touched <= limit);
    let lines_changed = metrics.lines_changed();
    let lines_passed = assertion
        .max_lines_changed
        .is_none_or(|limit| lines_changed <= limit);

    let files_evidence = assertion.max_files_touched.map_or_else(
        || format!("files touched: {} (no limit)", metrics.files_touched),
        |limit| {
            let comparison = if metrics.files_touched <= limit {
                "<="
            } else {
                ">"
            };
            format!(
                "files touched: {} {comparison} {limit}",
                metrics.files_touched
            )
        },
    );
    let lines_evidence = assertion.max_lines_changed.map_or_else(
        || {
            format!(
                "lines changed: {lines_changed} ({} added + {} removed; no limit)",
                metrics.lines_added, metrics.lines_removed
            )
        },
        |limit| {
            let comparison = if lines_changed <= limit { "<=" } else { ">" };
            format!(
                "lines changed: {lines_changed} {comparison} {limit} ({} added + {} removed)",
                metrics.lines_added, metrics.lines_removed
            )
        },
    );

    AssertionResult {
        id: assertion.id.clone(),
        passed: files_passed && lines_passed,
        evidence: format!(
            "{files_evidence}; {lines_evidence}; hunks: {}",
            metrics.hunks
        ),
        confidence: Some(1.0),
        grader: Some(Grader::DiffScope),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assertion(files: Option<u64>, lines: Option<u64>) -> AssertionDiffScope {
        AssertionDiffScope {
            id: "focused-change".to_string(),
            max_files_touched: files,
            max_lines_changed: lines,
        }
    }

    #[test]
    fn passes_when_all_configured_thresholds_are_met() {
        let result = grade_diff_scope(
            &assertion(Some(2), Some(5)),
            DiffScopeMetrics {
                files_touched: 2,
                lines_added: 3,
                lines_removed: 2,
                hunks: 2,
            },
        );

        assert!(result.passed);
        assert_eq!(result.grader, Some(Grader::DiffScope));
        assert_eq!(result.confidence, Some(1.0));
        assert!(result.evidence.contains("files touched: 2 <= 2"));
        assert!(result.evidence.contains("lines changed: 5 <= 5"));
        assert!(result.evidence.contains("3 added + 2 removed"));
    }

    #[test]
    fn fails_when_any_configured_threshold_is_exceeded() {
        let result = grade_diff_scope(
            &assertion(Some(1), Some(4)),
            DiffScopeMetrics {
                files_touched: 2,
                lines_added: 3,
                lines_removed: 2,
                hunks: 2,
            },
        );

        assert!(!result.passed);
        assert!(result.evidence.contains("files touched: 2 > 1"));
        assert!(result.evidence.contains("lines changed: 5 > 4"));
        assert!(result.evidence.contains("hunks: 2"));
    }

    #[test]
    fn only_configured_thresholds_gate_the_result() {
        let files_only = grade_diff_scope(
            &assertion(Some(1), None),
            DiffScopeMetrics {
                files_touched: 1,
                lines_added: 100,
                lines_removed: 100,
                hunks: 50,
            },
        );
        assert!(files_only.passed);

        let lines_only = grade_diff_scope(
            &assertion(None, Some(1)),
            DiffScopeMetrics {
                files_touched: 100,
                lines_added: 1,
                lines_removed: 0,
                hunks: 1,
            },
        );
        assert!(lines_only.passed);
    }
}
