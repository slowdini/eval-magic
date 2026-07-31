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
    legacy_shadowed: Option<Vec<LegacyShadowSource>>,
}

impl PluginShadowArtifact {
    pub(crate) fn new(report: PluginShadowReport, isolates_live_sources: bool) -> Self {
        Self {
            report,
            isolates_live_sources,
            legacy_shadowed: None,
        }
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
                Ok(Self::new(
                    PluginShadowReport {
                        config_dir: artifact.config_dir,
                        findings: artifact.findings,
                    },
                    artifact.isolates_live_sources,
                ))
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
pub(crate) fn format_isolated_shadow_notice(report: &PluginShadowReport) -> String {
    let count = report.findings.len();
    let finding = if count == 1 { "finding" } else { "findings" };
    [
        String::new(),
        format!("ℹ Skill-shadow notice: preflight detected {count} live-source {finding}."),
        "  The resolved descriptor declares `[shadow] isolates_live_sources = true`, so"
            .to_string(),
        "  the findings remain in plugin-shadow.json as informational provenance and".to_string(),
        "  will not become benchmark.json validity_warnings. eval-magic does not verify"
            .to_string(),
        "  this assertion; it must cover every initial and resumed eval-agent dispatch."
            .to_string(),
    ]
    .join("\n")
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

/// One `validity_warnings` entry per grouped logical skill.
pub fn shadow_validity_warnings(report: &PluginShadowReport) -> Vec<String> {
    report
        .findings
        .iter()
        .map(|finding| {
            let severity = severity_label(finding.severity);
            let sources = finding
                .sources
                .iter()
                .filter(|source| source.origin == ShadowSourceOrigin::Live)
                .map(source_label)
                .collect::<Vec<_>>()
                .join(", ");
            let remediation = finding
                .sources
                .iter()
                .filter_map(|source| source.remediation.as_deref())
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "{severity}: staged {} skill '{}' is also discoverable from {sources}. {remediation}",
                role_label(finding.role),
                finding.skill_name
            )
            .trim()
            .to_string()
        })
        .collect()
}

fn legacy_shadow_validity_warnings(sources: &[LegacyShadowSource]) -> Vec<String> {
    const ISOLATION_DOC: &str = "docs/claude-notes.md → \"Isolating from installed plugins\"";
    sources
        .iter()
        .map(|source| {
            format!(
                "staged skill '{}' is also provided by {} — each claude -p dispatch could discover \
                 both copies, so with/without results may be contaminated. Isolate each dispatch's \
                 Claude config (see {}).",
                source.skill_name(),
                source.source_label(),
                ISOLATION_DOC
            )
        })
        .collect()
}

/// Shared build-time banner. Empty when nothing is shadowed.
pub fn format_shadow_banner(report: &PluginShadowReport) -> String {
    if report.findings.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        String::new(),
        "⚠ Skill-shadow preflight found live copies of staged eval skills:".to_string(),
    ];
    for finding in &report.findings {
        lines.push(format!(
            "  • [{}] {} ({})",
            severity_label(finding.severity),
            finding.skill_name,
            role_label(finding.role)
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
    lines.join("\n")
}

fn severity_label(severity: ShadowSeverity) -> &'static str {
    match severity {
        ShadowSeverity::Warning => "warning",
        ShadowSeverity::ComparisonInvalid => "comparison invalid",
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
