//! User-facing rendering for persisted shadow reports.

use super::*;

/// Informational build-time notice for a report whose resolved descriptor
/// asserts that every detected live source is isolated from dispatches.
pub(crate) fn format_isolated_shadow_notice(report: &PluginShadowReport, verifies: bool) -> String {
    let count = report.findings.len();
    let finding = if count == 1 { "finding" } else { "findings" };
    let mut lines = vec![
        String::new(),
        format!("ℹ Skill-shadow notice: preflight detected {count} live-source {finding}."),
        "  The resolved descriptor declares `[shadow] isolates_live_sources = true`, so"
            .to_string(),
        "  the findings remain in plugin-shadow.json as informational provenance and".to_string(),
        "  will not become benchmark.json validity_warnings.".to_string(),
    ];
    lines.push(if verifies {
        // Saying "eval-magic does not verify this" would now be false for this
        // harness: `ingest` checks the assertion against the transcripts.
        "  `ingest` checks this assertion against what each dispatch reported, and".to_string()
    } else {
        "  eval-magic cannot verify this assertion for this harness; it must cover".to_string()
    });
    lines.push(if verifies {
        "  `aggregate` reports any contradiction.".to_string()
    } else {
        "  every initial and resumed eval-agent dispatch.".to_string()
    });
    lines.push("  How to confirm it holds: `eval-magic docs isolation`.".to_string());
    lines.join("\n")
}

fn source_label(source: &ShadowSource) -> String {
    match &source.plugin {
        Some(plugin) => format!("enabled plugin '{plugin}'"),
        None => format!(
            "{} {:?} skill root '{}'",
            relation_label(source.root.relation),
            source.root.scope,
            source.root.path
        ),
    }
}

/// Join distinct entries in first-seen order — two cached versions of one
/// installed plugin yield separate sources sharing a label and a remediation, so
/// joining verbatim says the same thing twice. Order-preserving rather than
/// sorted: these strings land in `benchmark.json` and must match the order the
/// banner prints its sources in.
fn join_distinct(values: impl Iterator<Item = String>, separator: &str) -> String {
    let mut distinct: Vec<String> = Vec::new();
    for value in values {
        if !distinct.contains(&value) {
            distinct.push(value);
        }
    }
    distinct.join(separator)
}

/// One `validity_warnings` entry per grouped logical skill.
///
/// A finding whose evidence refuted every live source produces **nothing**: the
/// dispatches demonstrably did not load it, so there is no threat to report.
/// Everything else reports, and says which of the three it is — confirmed by
/// transcripts, or detected but unverifiable — because "we saw it happen" and
/// "we could not tell" call for different responses from the operator.
pub fn shadow_validity_warnings(report: &PluginShadowReport) -> Vec<String> {
    report
        .findings
        .iter()
        .filter(|finding| {
            finding.resolved_severity != Some(verification::ShadowResolvedSeverity::Isolated)
        })
        .map(|finding| {
            let sources = join_distinct(
                finding
                    .sources
                    .iter()
                    .filter(|source| source.origin == ShadowSourceOrigin::Live)
                    .map(source_label),
                ", ",
            );
            let remediation = join_distinct(
                finding
                    .sources
                    .iter()
                    .filter_map(|source| source.remediation.clone()),
                " ",
            );
            let role = role_label(finding.role);
            let skill = &finding.skill_name;
            let lead = match finding.resolved_severity {
                Some(resolved) => {
                    let severity = resolved_severity_label(resolved);
                    match verification::finding_status(finding) {
                        verification::VerificationStatus::Confirmed => {
                            let cells = verification::confirmed_cells(finding).join(", ");
                            let n = verification::confirming_dispatch_count(finding);
                            let plural = if n == 1 { "" } else { "s" };
                            format!(
                                "{severity}: staged {role} skill '{skill}' was actually loaded \
                                 from {sources} in {cells} (verified from {n} dispatch \
                                 transcript{plural})."
                            )
                        }
                        _ => unverified_lead(severity, role, skill, &sources, finding),
                    }
                }
                None => format!(
                    "{}: staged {role} skill '{skill}' is also discoverable from {sources}.",
                    severity_label(finding.severity)
                ),
            };
            format!("{lead} {remediation} See `eval-magic docs isolation`.")
                .trim()
                .to_string()
        })
        .collect()
}

/// The lead sentence for a finding evidence could not settle, naming why.
fn unverified_lead(
    severity: &str,
    role: &str,
    skill: &str,
    sources: &str,
    finding: &ShadowFinding,
) -> String {
    let reason = verification::inconclusive_reason(finding)
        .unwrap_or_else(|| "no dispatch reported its skill/plugin surface".to_string());
    format!(
        "{severity} (unverified): staged {role} skill '{skill}' is discoverable from {sources}, \
         and eval-magic could not verify whether dispatches loaded it — {reason}. Treat the \
         comparison as affected until each dispatch isolates the source or a transcript shows it \
         did not load."
    )
}

pub(super) fn legacy_shadow_validity_warnings(sources: &[LegacyShadowSource]) -> Vec<String> {
    sources
        .iter()
        .map(|source| {
            format!(
                "staged skill '{}' is also provided by {} — each claude -p dispatch could discover \
                 both copies, so with/without results may be contaminated. Isolate each dispatch's \
                 Claude config: add --setting-sources project,local to drop user-scope plugins, \
                 disable the plugin in enabledPlugins settings, or run under a clean \
                 CLAUDE_CONFIG_DIR.",
                source.skill_name(),
                source.source_label(),
            )
        })
        .collect()
}

/// Shared build-time banner. Empty when nothing is shadowed.
///
/// Nothing has dispatched yet, so this states a *risk*, not a verdict. Whether a
/// dispatch actually loads one of these depends on its own config isolation,
/// which eval-magic reads from the transcript during `ingest` rather than
/// inferring from command templates. Printing "comparison invalid" here would
/// convict a correctly-isolated run before it ran a single task (issue #207).
///
/// `verifies` reflects whether this harness's transcripts can settle it.
pub fn format_shadow_banner_with_verification(
    report: &PluginShadowReport,
    verifies: bool,
) -> String {
    if report.findings.is_empty() {
        return String::new();
    }
    let has_operator = report
        .findings
        .iter()
        .any(|finding| finding.class == ShadowFindingClass::OperatorEnvironment);
    let has_codebase = report
        .findings
        .iter()
        .any(|finding| finding.class == ShadowFindingClass::CodebaseSourced);
    let mut lines = vec![String::new()];
    match (has_operator, has_codebase) {
        (true, false) => lines.extend([
            "⚠ Skill-shadow preflight: live copies of evaluated skills are installed in this"
                .to_string(),
            "  operator environment. Whether a dispatch loads them depends on that dispatch's own"
                .to_string(),
            "  config isolation. At risk unless every dispatch isolates these sources:"
                .to_string(),
        ]),
        (false, true) => lines.extend([
            "⚠ Skill-shadow preflight: project-local copies of evaluated skills remain discoverable"
                .to_string(),
            "  from the sourced codebase. At risk unless those sources are excluded or displaced:"
                .to_string(),
        ]),
        (true, true) => lines.extend([
            "⚠ Skill-shadow preflight: evaluated skills have additional discoverable copies in the"
                .to_string(),
            "  operator environment and the sourced codebase. At risk unless every source is isolated"
                .to_string(),
            "  or excluded:".to_string(),
        ]),
        (false, false) => unreachable!("an empty report returned above"),
    }
    for finding in &report.findings {
        lines.push(format!(
            "  • [{}] {} — {}",
            role_label(finding.role),
            finding.skill_name,
            consequence(finding.severity)
        ));
        for source in finding
            .sources
            .iter()
            .filter(|source| source.origin == ShadowSourceOrigin::Live)
        {
            lines.push(format!(
                "    - {} [{}; runtime id '{}']",
                source_label(source),
                relation_label(source.root.relation),
                source.runtime_id
            ));
            if !source.appearances.is_empty() {
                let cells = source
                    .appearances
                    .iter()
                    .map(|appearance| {
                        format!(
                            "{}/{} ({})",
                            appearance.group,
                            appearance.condition,
                            resolution_label(appearance.resolution)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!("      expected in: {cells}"));
            }
            if let Some(remediation) = &source.remediation {
                lines.push(format!("      remediation: {remediation}"));
            }
        }
    }
    lines.push("  See plugin-shadow.json for canonical paths and full provenance.".to_string());
    lines.push(if verifies {
        "  `ingest` records what each dispatch actually loaded and `aggregate` reports the \
         verified verdict."
            .to_string()
    } else {
        "  This harness's transcripts do not report the session's skill/plugin surface, so \
         eval-magic cannot verify isolation."
            .to_string()
    });
    lines.push(
        "  Per-harness isolation recipes, and how to verify one worked: \
         `eval-magic docs isolation`."
            .to_string(),
    );
    lines.join("\n")
}

/// Banner for a caller with no harness context. Defaults to "cannot verify",
/// the conservative direction: understating what eval-magic can settle is safe,
/// promising a verdict it will never produce is not. Prefer
/// [`format_shadow_banner_with_verification`] wherever the harness is known.
pub fn format_shadow_banner(report: &PluginShadowReport) -> String {
    format_shadow_banner_with_verification(report, false)
}

/// What the collision would cost if the live copy did load. Conditional mood on
/// purpose — at banner time it has not happened yet.
fn consequence(severity: ShadowSeverity) -> &'static str {
    match severity {
        ShadowSeverity::Warning => "would weaken the comparison if loaded",
        ShadowSeverity::ComparisonInvalid => "would invalidate the comparison if loaded",
    }
}

fn severity_label(severity: ShadowSeverity) -> &'static str {
    match severity {
        ShadowSeverity::Warning => "warning",
        ShadowSeverity::ComparisonInvalid => "comparison invalid",
    }
}

fn resolved_severity_label(severity: verification::ShadowResolvedSeverity) -> &'static str {
    match severity {
        verification::ShadowResolvedSeverity::Isolated => "isolated",
        verification::ShadowResolvedSeverity::Warning => "warning",
        verification::ShadowResolvedSeverity::ComparisonInvalid => "comparison invalid",
    }
}

fn role_label(role: ShadowSkillRole) -> &'static str {
    match role {
        ShadowSkillRole::Subject => "subject",
        ShadowSkillRole::Sibling => "sibling",
    }
}

fn relation_label(relation: ShadowRelation) -> &'static str {
    match relation {
        ShadowRelation::Native => "native",
        ShadowRelation::CrossHarness => "cross-harness",
        ShadowRelation::Unknown => "unknown",
    }
}

fn resolution_label(resolution: ShadowResolution) -> &'static str {
    match resolution {
        ShadowResolution::Selected => "selected",
        ShadowResolution::Shadowed => "shadowed",
        ShadowResolution::Coexisting => "coexisting",
        ShadowResolution::Unknown => "unknown",
    }
}
