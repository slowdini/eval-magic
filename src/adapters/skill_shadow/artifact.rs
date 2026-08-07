//! Persistence compatibility and user-facing rendering for shadow reports.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::*;

/// The persisted v2 artifact. Deserialization also accepts the historical,
/// unversioned `shadowed` shape so old iterations remain aggregatable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginShadowArtifact {
    pub report: PluginShadowReport,
    pub isolates_live_sources: bool,
    /// Run-level verification totals, written by `ingest` once transcripts exist.
    /// Absent on an artifact `run` just wrote, and on a harness that reports no
    /// session surface.
    pub verification: Option<ReportVerification>,
    legacy_shadowed: Option<Vec<LegacyShadowSource>>,
}

impl PluginShadowArtifact {
    pub(crate) fn new(report: PluginShadowReport, isolates_live_sources: bool) -> Self {
        Self {
            report,
            isolates_live_sources,
            verification: None,
            legacy_shadowed: None,
        }
    }

    /// A legacy unversioned artifact carries no per-cell appearances, so there is
    /// nothing to join evidence against — it can never be verified.
    pub(crate) fn is_legacy(&self) -> bool {
        self.legacy_shadowed.is_some()
    }

    pub(crate) fn validity_warnings(&self) -> Vec<String> {
        self.legacy_shadowed.as_ref().map_or_else(
            || shadow_validity_warnings(&self.report),
            |sources| legacy_shadow_validity_warnings(sources),
        )
    }
}

#[derive(Serialize)]
struct PluginShadowArtifactRef<'a> {
    schema_version: u8,
    config_dir: &'a str,
    findings: &'a [ShadowFinding],
    #[serde(skip_serializing_if = "is_false")]
    isolates_live_sources: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification: &'a Option<ReportVerification>,
}

impl Serialize for PluginShadowArtifact {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        PluginShadowArtifactRef {
            schema_version: PLUGIN_SHADOW_SCHEMA_VERSION,
            config_dir: &self.report.config_dir,
            findings: &self.report.findings,
            isolates_live_sources: self.isolates_live_sources,
            verification: &self.verification,
        }
        .serialize(serializer)
    }
}

#[derive(Deserialize)]
struct PluginShadowArtifactV2 {
    schema_version: u8,
    config_dir: String,
    findings: Vec<ShadowFinding>,
    #[serde(default)]
    isolates_live_sources: bool,
    #[serde(default)]
    verification: Option<ReportVerification>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum LegacyShadowSource {
    Plugin {
        plugin: String,
        skill_name: String,
        path: String,
    },
    GlobalSkill {
        skill_name: String,
        path: String,
    },
}

impl LegacyShadowSource {
    fn skill_name(&self) -> &str {
        match self {
            Self::Plugin { skill_name, .. } | Self::GlobalSkill { skill_name, .. } => skill_name,
        }
    }

    fn source_label(&self) -> String {
        match self {
            Self::Plugin { plugin, .. } => format!("enabled plugin '{plugin}'"),
            Self::GlobalSkill { .. } => "the global skills dir".to_string(),
        }
    }

    fn to_v2(&self) -> ShadowSource {
        match self {
            Self::Plugin {
                plugin,
                skill_name,
                path,
            } => ShadowSource {
                kind: ShadowSourceKind::Plugin,
                origin: ShadowSourceOrigin::Live,
                skill_name: skill_name.clone(),
                runtime_id: skill_name.clone(),
                plugin: Some(plugin.clone()),
                discovery_path: path.clone(),
                canonical_path: None,
                root: ShadowRoot::unknown(path),
                appearances: Vec::new(),
                remediation: None,
                verification: None,
            },
            Self::GlobalSkill { skill_name, path } => ShadowSource {
                kind: ShadowSourceKind::Skill,
                origin: ShadowSourceOrigin::Live,
                skill_name: skill_name.clone(),
                runtime_id: skill_name.clone(),
                plugin: None,
                discovery_path: path.clone(),
                canonical_path: None,
                root: ShadowRoot::unknown(path),
                appearances: Vec::new(),
                remediation: None,
                verification: None,
            },
        }
    }
}

#[derive(Deserialize)]
struct LegacyPluginShadowArtifact {
    config_dir: String,
    shadowed: Vec<LegacyShadowSource>,
    #[serde(default)]
    isolates_live_sources: bool,
}

impl<'de> Deserialize<'de> for PluginShadowArtifact {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value.get("schema_version").and_then(|value| value.as_u64()) {
            Some(version) if version == u64::from(PLUGIN_SHADOW_SCHEMA_VERSION) => {
                let artifact: PluginShadowArtifactV2 =
                    serde_json::from_value(value).map_err(D::Error::custom)?;
                if artifact.schema_version != PLUGIN_SHADOW_SCHEMA_VERSION {
                    return Err(D::Error::custom("unsupported plugin-shadow schema version"));
                }
                Ok(Self {
                    report: PluginShadowReport {
                        config_dir: artifact.config_dir,
                        findings: artifact.findings,
                    },
                    isolates_live_sources: artifact.isolates_live_sources,
                    verification: artifact.verification,
                    legacy_shadowed: None,
                })
            }
            Some(version) => Err(D::Error::custom(format!(
                "unsupported plugin-shadow schema version {version}"
            ))),
            None => {
                let legacy: LegacyPluginShadowArtifact =
                    serde_json::from_value(value).map_err(D::Error::custom)?;
                let report = PluginShadowReport::from_sources(
                    legacy.config_dir,
                    legacy
                        .shadowed
                        .iter()
                        .map(LegacyShadowSource::to_v2)
                        .collect(),
                );
                Ok(Self {
                    report,
                    isolates_live_sources: legacy.isolates_live_sources,
                    verification: None,
                    legacy_shadowed: Some(legacy.shadowed),
                })
            }
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

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

fn legacy_shadow_validity_warnings(sources: &[LegacyShadowSource]) -> Vec<String> {
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
    let mut lines = vec![
        String::new(),
        "⚠ Skill-shadow preflight: live copies of staged eval skills are installed in this"
            .to_string(),
        "  operator environment. Whether a dispatch loads them depends on that dispatch's own"
            .to_string(),
        "  config isolation. At risk unless every dispatch isolates these sources:".to_string(),
    ];
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
