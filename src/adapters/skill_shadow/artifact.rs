//! Persistence compatibility and user-facing rendering for shadow reports.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::*;

mod render;

pub(crate) use render::format_isolated_shadow_notice;
use render::legacy_shadow_validity_warnings;
pub use render::{
    format_shadow_banner, format_shadow_banner_with_verification, shadow_validity_warnings,
};

/// The persisted v3 artifact. Deserialization also accepts schema v2 and the
/// unversioned `shadowed` shape so artifacts from that contract remain aggregatable.
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

    pub(crate) fn validity_warnings_for_class(&self, class: ShadowFindingClass) -> Vec<String> {
        if let Some(sources) = &self.legacy_shadowed {
            return if class == ShadowFindingClass::OperatorEnvironment {
                legacy_shadow_validity_warnings(sources)
            } else {
                Vec::new()
            };
        }
        shadow_validity_warnings(&PluginShadowReport {
            config_dir: self.report.config_dir.clone(),
            findings: self
                .report
                .findings
                .iter()
                .filter(|finding| finding.class == class)
                .cloned()
                .collect(),
        })
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
struct VersionedPluginShadowArtifact {
    #[serde(rename = "schema_version")]
    _schema_version: u8,
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
            Some(2 | 3) => {
                let artifact: VersionedPluginShadowArtifact =
                    serde_json::from_value(value).map_err(D::Error::custom)?;
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
