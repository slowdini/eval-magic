//! Harness-neutral skill-shadow report types, grouping, severity, persistence,
//! and rendering.
//!
//! Harness scanners contribute facts about live discovery sources. This module
//! owns the stable report contract and turns those facts into one finding per
//! logical eval skill. A source's harness-facing `runtime_id` is deliberately
//! distinct from the logical `skill_name`: staged slugs and namespaced plugin
//! skills can coexist with a live logical copy without sharing an identifier.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::fs::artifact_path;

mod artifact;
mod resolution;
pub(crate) mod verification;

pub(crate) use artifact::{PluginShadowArtifact, format_isolated_shadow_notice};
pub use artifact::{
    format_shadow_banner, format_shadow_banner_with_verification, shadow_validity_warnings,
};
pub(crate) use resolution::{
    resolve_as_coexisting, resolve_by_precedence, resolve_from_selected_paths,
};
pub use verification::{
    CellVerification, ReportVerification, ShadowResolvedSeverity, SourceVerification,
    VerificationStatus,
};

pub const PLUGIN_SHADOW_SCHEMA_VERSION: u8 = 2;

/// How a logical skill participates in the comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowSkillRole {
    Subject,
    Sibling,
}

/// The intrinsic severity before a descriptor's isolation assertion is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowSeverity {
    Warning,
    ComparisonInvalid,
}

/// The concrete form of a discovered skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowSourceKind {
    Skill,
    Plugin,
}

/// Whether the source is runner-controlled or inherited from the live environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowSourceOrigin {
    Live,
    Staged,
}

/// Filesystem scope of a discovery root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowRootScope {
    Project,
    Global,
    Admin,
    Unknown,
}

/// Convention or plugin namespace that owns the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowNamespace {
    Opencode,
    Claude,
    Cline,
    Agents,
    Codex,
    Plugin,
    Unknown,
}

/// Whether a harness owns the root convention it discovers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowRelation {
    Native,
    CrossHarness,
    Unknown,
}

/// What the harness does with one source when duplicate runtime ids exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowResolution {
    Selected,
    Shadowed,
    Coexisting,
    Unknown,
}

/// The discovery root that contributed a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowRoot {
    pub scope: ShadowRootScope,
    pub namespace: ShadowNamespace,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    pub path: String,
    pub relation: ShadowRelation,
}

impl ShadowRoot {
    pub(crate) fn unknown(path: impl Into<String>) -> Self {
        Self {
            scope: ShadowRootScope::Unknown,
            namespace: ShadowNamespace::Unknown,
            plugin: None,
            path: path.into(),
            relation: ShadowRelation::Unknown,
        }
    }

    pub(crate) fn staged(skills_dir: &Path) -> Self {
        let namespace = match skills_dir
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
        {
            Some(".claude") => ShadowNamespace::Claude,
            Some(".agents") => ShadowNamespace::Agents,
            Some(".opencode") => ShadowNamespace::Opencode,
            Some(".codex") => ShadowNamespace::Codex,
            _ => ShadowNamespace::Unknown,
        };
        Self {
            scope: ShadowRootScope::Project,
            namespace,
            plugin: None,
            path: artifact_path(skills_dir),
            relation: ShadowRelation::Native,
        }
    }
}

/// One comparison cell where a source is expected to be discoverable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowAppearance {
    pub group: String,
    pub condition: String,
    pub eval_ids: Vec<String>,
    pub resolution: ShadowResolution,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precedence_rank: Option<u32>,
}

/// One concrete copy of a logical skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowSource {
    pub kind: ShadowSourceKind,
    pub origin: ShadowSourceOrigin,
    pub skill_name: String,
    pub runtime_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    pub discovery_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_path: Option<String>,
    pub root: ShadowRoot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub appearances: Vec<ShadowAppearance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    /// What dispatch transcripts showed about this source. Absent until `ingest`
    /// reconciles the finding, and absent forever on a harness whose transcripts
    /// carry no skill/plugin roster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<verification::SourceVerification>,
}

impl ShadowSource {
    pub(crate) fn live_skill(
        skill_name: impl Into<String>,
        path: &Path,
        root: ShadowRoot,
        remediation: impl Into<String>,
    ) -> Self {
        let skill_name = skill_name.into();
        Self {
            kind: ShadowSourceKind::Skill,
            origin: ShadowSourceOrigin::Live,
            runtime_id: skill_name.clone(),
            skill_name,
            plugin: None,
            discovery_path: artifact_path(path),
            canonical_path: canonical_path(path),
            root,
            appearances: Vec::new(),
            remediation: Some(remediation.into()),
            verification: None,
        }
    }

    pub(crate) fn live_plugin(
        plugin: impl Into<String>,
        skill_name: impl Into<String>,
        runtime_id: impl Into<String>,
        path: &Path,
        root: ShadowRoot,
        remediation: impl Into<String>,
    ) -> Self {
        Self {
            kind: ShadowSourceKind::Plugin,
            origin: ShadowSourceOrigin::Live,
            skill_name: skill_name.into(),
            runtime_id: runtime_id.into(),
            plugin: Some(plugin.into()),
            discovery_path: artifact_path(path),
            canonical_path: canonical_path(path),
            root,
            appearances: Vec::new(),
            remediation: Some(remediation.into()),
            verification: None,
        }
    }

    pub(crate) fn staged(
        skill_name: impl Into<String>,
        runtime_id: impl Into<String>,
        path: &Path,
        root: ShadowRoot,
    ) -> Self {
        Self {
            kind: ShadowSourceKind::Skill,
            origin: ShadowSourceOrigin::Staged,
            skill_name: skill_name.into(),
            runtime_id: runtime_id.into(),
            plugin: None,
            discovery_path: artifact_path(path),
            canonical_path: canonical_path(path),
            root,
            appearances: Vec::new(),
            remediation: None,
            verification: None,
        }
    }

    pub(crate) fn skill_name(&self) -> &str {
        &self.skill_name
    }

    pub(crate) fn add_appearance(&mut self, mut appearance: ShadowAppearance) {
        if let Some(existing) = self.appearances.iter_mut().find(|existing| {
            existing.group == appearance.group
                && existing.condition == appearance.condition
                && existing.resolution == appearance.resolution
                && existing.precedence_rank == appearance.precedence_rank
        }) {
            existing.eval_ids.append(&mut appearance.eval_ids);
            existing.eval_ids.sort();
            existing.eval_ids.dedup();
        } else {
            appearance.eval_ids.sort();
            appearance.eval_ids.dedup();
            self.appearances.push(appearance);
            self.appearances
                .sort_by(|a, b| (&a.group, &a.condition).cmp(&(&b.group, &b.condition)));
        }
    }

    fn same_identity(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.origin == other.origin
            && self.skill_name == other.skill_name
            && self.runtime_id == other.runtime_id
            && self.plugin == other.plugin
            && self.discovery_path == other.discovery_path
            && self.canonical_path == other.canonical_path
            && self.root == other.root
            && self.remediation == other.remediation
    }
}

/// The resolved real path, rendered in the artifact wire format shared by
/// agents and reviewers.
fn canonical_path(path: &Path) -> Option<String> {
    path.canonicalize().ok().map(|path| artifact_path(&path))
}

/// Severity for one finding, given the cells its live sources appear in.
///
/// A subject collision is always comparison-invalid: both arms can resolve the
/// live copy, so the delta measures nothing. A sibling collision is a mere
/// warning only when it is *symmetric* — present in every condition of a group or
/// none of them — because asymmetric contamination is indistinguishable from the
/// effect under test.
///
/// Shared by build-time detection and after-the-fact verification, which apply it
/// to different cell sets: detection passes every cell a source was expected in,
/// verification passes only the cells a transcript confirmed. Keeping one
/// implementation is what stops the two verdicts from drifting apart.
pub(crate) fn severity_for(
    role: ShadowSkillRole,
    live_cells: &BTreeSet<(&str, &str)>,
    expected_by_group: &BTreeMap<&str, BTreeSet<&str>>,
) -> ShadowSeverity {
    match role {
        ShadowSkillRole::Subject => ShadowSeverity::ComparisonInvalid,
        ShadowSkillRole::Sibling => {
            let symmetric = expected_by_group.iter().all(|(group, conditions)| {
                let seen = conditions
                    .iter()
                    .filter(|condition| live_cells.contains(&(*group, *condition)))
                    .count();
                seen == 0 || seen == conditions.len()
            });
            if symmetric {
                ShadowSeverity::Warning
            } else {
                ShadowSeverity::ComparisonInvalid
            }
        }
    }
}

/// Every concrete source associated with one logical eval skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowFinding {
    pub skill_name: String,
    pub role: ShadowSkillRole,
    /// What the collision would mean if the live copy loaded. Set at detection
    /// time and **never** rewritten — evidence resolves
    /// [`resolved_severity`](Self::resolved_severity) instead, so the artifact
    /// keeps both the risk and the outcome.
    pub severity: ShadowSeverity,
    pub sources: Vec<ShadowSource>,
    /// Severity after transcript evidence is applied. `None` means the finding
    /// was never verified, which is distinct from being verified and found
    /// harmless (`Some(Isolated)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_severity: Option<verification::ShadowResolvedSeverity>,
}

/// The detector's grouped findings for a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginShadowReport {
    pub config_dir: String,
    pub findings: Vec<ShadowFinding>,
}

impl PluginShadowReport {
    pub(crate) fn from_sources(config_dir: impl Into<String>, sources: Vec<ShadowSource>) -> Self {
        let mut grouped: BTreeMap<String, Vec<ShadowSource>> = BTreeMap::new();
        for source in sources {
            grouped
                .entry(source.skill_name.clone())
                .or_default()
                .push(source);
        }
        let findings = grouped
            .into_iter()
            .map(|(skill_name, mut sources)| {
                sources.sort_by(|a, b| {
                    (
                        &a.runtime_id,
                        &a.discovery_path,
                        a.origin == ShadowSourceOrigin::Staged,
                    )
                        .cmp(&(
                            &b.runtime_id,
                            &b.discovery_path,
                            b.origin == ShadowSourceOrigin::Staged,
                        ))
                });
                ShadowFinding {
                    skill_name,
                    role: ShadowSkillRole::Subject,
                    severity: ShadowSeverity::ComparisonInvalid,
                    sources,
                    resolved_severity: None,
                }
            })
            .collect();
        Self {
            config_dir: config_dir.into(),
            findings,
        }
    }

    pub(crate) fn from_observed_sources(
        config_dir: impl Into<String>,
        sources: Vec<ShadowSource>,
        subject_skill_name: &str,
        expected_cells: &[(String, String)],
    ) -> Self {
        let mut merged = Vec::<ShadowSource>::new();
        for mut source in sources {
            if let Some(existing) = merged
                .iter_mut()
                .find(|existing| existing.same_identity(&source))
            {
                for appearance in source.appearances.drain(..) {
                    existing.add_appearance(appearance);
                }
            } else {
                merged.push(source);
            }
        }

        let mut report = Self::from_sources(config_dir, merged);
        let mut expected_by_group = BTreeMap::<&str, BTreeSet<&str>>::new();
        for (group, condition) in expected_cells {
            expected_by_group
                .entry(group)
                .or_default()
                .insert(condition);
        }
        for finding in &mut report.findings {
            finding.role = if finding.skill_name == subject_skill_name {
                ShadowSkillRole::Subject
            } else {
                ShadowSkillRole::Sibling
            };
            let live_cells = finding
                .sources
                .iter()
                .filter(|source| source.origin == ShadowSourceOrigin::Live)
                .flat_map(|source| &source.appearances)
                .map(|appearance| (appearance.group.as_str(), appearance.condition.as_str()))
                .collect::<BTreeSet<_>>();
            finding.severity = severity_for(finding.role, &live_cells, &expected_by_group);
        }
        report
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn source_count(&self) -> usize {
        self.findings
            .iter()
            .map(|finding| finding.sources.len())
            .sum()
    }

    #[cfg(test)]
    pub(crate) fn sources(&self) -> impl Iterator<Item = &ShadowSource> {
        self.findings.iter().flat_map(|finding| &finding.sources)
    }

    pub(crate) fn into_sources(self) -> Vec<ShadowSource> {
        self.findings
            .into_iter()
            .flat_map(|finding| finding.sources)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn source(&self, index: usize) -> &ShadowSource {
        self.sources()
            .nth(index)
            .expect("shadow source index should exist")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> PluginShadowReport {
        PluginShadowReport::from_sources(
            "/x",
            vec![ShadowSource {
                kind: ShadowSourceKind::Plugin,
                origin: ShadowSourceOrigin::Live,
                skill_name: "verification-before-completion".into(),
                runtime_id: "slow-powers:verification-before-completion".into(),
                plugin: Some("slow-powers@slowdini".into()),
                discovery_path: "/p".into(),
                canonical_path: Some("/p".into()),
                root: ShadowRoot {
                    scope: ShadowRootScope::Global,
                    namespace: ShadowNamespace::Plugin,
                    plugin: Some("slow-powers@slowdini".into()),
                    path: "/plugins/slow-powers".into(),
                    relation: ShadowRelation::Native,
                },
                appearances: vec![ShadowAppearance {
                    group: "g1".into(),
                    condition: "without_skill".into(),
                    eval_ids: vec!["e1".into()],
                    resolution: ShadowResolution::Coexisting,
                    precedence_rank: None,
                }],
                remediation: Some(
                    "Disable plugin 'slow-powers@slowdini' for every dispatch.".into(),
                ),
                verification: None,
            }],
        )
    }

    #[test]
    fn legacy_shadow_artifact_defaults_to_undeclared_isolation() {
        let artifact: PluginShadowArtifact = serde_json::from_value(serde_json::json!({
            "config_dir": "/x",
            "shadowed": [{
                "kind": "plugin",
                "plugin": "slow-powers@slowdini",
                "skill_name": "mr-review",
                "path": "/x/plugin/skills/mr-review"
            }]
        }))
        .unwrap();

        assert!(!artifact.isolates_live_sources);
        assert_eq!(artifact.report.findings.len(), 1);
        assert_eq!(artifact.validity_warnings().len(), 1);
        assert!(
            serde_json::to_value(&artifact)
                .unwrap()
                .get("shadowed")
                .is_none(),
            "legacy input is normalized to v2 when reserialized"
        );
    }

    #[test]
    fn declared_isolation_is_serialized_with_the_shadow_report() {
        let artifact = PluginShadowArtifact::new(sample_report(), true);
        let value = serde_json::to_value(&artifact).unwrap();
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["isolates_live_sources"], true);
        assert_eq!(
            value["findings"][0]["skill_name"],
            "verification-before-completion"
        );
    }

    #[test]
    fn validity_warnings_name_skill_plugin_and_severity() {
        let warnings = shadow_validity_warnings(&sample_report());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("verification-before-completion"));
        assert!(warnings[0].contains("slow-powers@slowdini"));
        assert!(warnings[0].contains("comparison invalid"));
    }

    /// Two cached versions of one installed plugin, each shipping the same skill
    /// — the shape a real plugin cache takes once an upgrade leaves both versions
    /// on disk. The scan contributes two distinct sources that share a plugin key
    /// and a remediation.
    fn report_with_one_plugin_cached_at_two_versions() -> PluginShadowReport {
        let source_at = |version: &str| ShadowSource {
            kind: ShadowSourceKind::Plugin,
            origin: ShadowSourceOrigin::Live,
            skill_name: "hardening-plans".into(),
            runtime_id: "slow-powers:hardening-plans".into(),
            plugin: Some("slow-powers@slowdini".into()),
            discovery_path: format!("/cache/slow-powers/{version}/skills/hardening-plans"),
            canonical_path: None,
            root: ShadowRoot {
                scope: ShadowRootScope::Global,
                namespace: ShadowNamespace::Plugin,
                plugin: Some("slow-powers@slowdini".into()),
                path: format!("/cache/slow-powers/{version}/skills"),
                relation: ShadowRelation::Native,
            },
            appearances: vec![],
            remediation: Some(
                "Disable plugin 'slow-powers@slowdini' in the effective enabledPlugins settings \
                 for every dispatch."
                    .into(),
            ),
            verification: None,
        };
        PluginShadowReport::from_sources("/x", vec![source_at("0.5.2"), source_at("0.5.4")])
    }

    #[test]
    fn duplicate_source_labels_and_remediations_are_deduped_in_validity_warnings() {
        let warnings = shadow_validity_warnings(&report_with_one_plugin_cached_at_two_versions());
        assert_eq!(warnings.len(), 1);
        let warning = &warnings[0];
        assert_eq!(
            warning
                .matches("enabled plugin 'slow-powers@slowdini'")
                .count(),
            1,
            "one plugin cached twice must be named once, not once per cached copy: {warning}"
        );
        assert_eq!(
            warning.matches("Disable plugin").count(),
            1,
            "an identical remediation must not repeat: {warning}"
        );
    }

    #[test]
    fn banner_is_empty_when_nothing_shadowed() {
        let empty = PluginShadowReport {
            config_dir: "/x".into(),
            findings: vec![],
        };
        assert_eq!(format_shadow_banner(&empty), "");
    }

    #[test]
    fn banner_lists_shadowed_skills_cells_and_per_source_remediation() {
        let banner = format_shadow_banner(&sample_report());
        assert!(banner.contains("verification-before-completion"));
        assert!(banner.contains("slow-powers@slowdini"));
        assert!(banner.contains("g1/without_skill"));
        assert!(banner.contains("Disable plugin"));
    }

    /// The banner and the isolated notice are what an operator reads at the
    /// moment they hit a shadow finding, so each has to name the topic that
    /// tells them what to do. Shipped output cites the embedded topic, never a
    /// repo-relative path a binary-only install cannot open.
    #[test]
    fn banner_and_isolated_notice_point_at_the_isolation_topic() {
        for rendered in [
            format_shadow_banner(&sample_report()),
            format_isolated_shadow_notice(&sample_report(), true),
        ] {
            assert!(
                rendered.contains("eval-magic docs isolation"),
                "must name the topic that explains the remedy: {rendered}"
            );
            assert!(
                !rendered.contains("docs/isolation.md"),
                "must not cite a repo path: {rendered}"
            );
        }
    }

    #[test]
    fn v2_artifact_groups_sources_under_one_logical_finding() {
        let artifact = PluginShadowArtifact::new(
            PluginShadowReport {
                config_dir: "/home/u/.config/opencode".into(),
                findings: vec![ShadowFinding {
                    skill_name: "mr-review".into(),
                    role: ShadowSkillRole::Subject,
                    severity: ShadowSeverity::ComparisonInvalid,
                    sources: vec![ShadowSource {
                        kind: ShadowSourceKind::Skill,
                        origin: ShadowSourceOrigin::Live,
                        skill_name: "mr-review".into(),
                        runtime_id: "mr-review".into(),
                        plugin: None,
                        discovery_path: "/home/u/.claude/skills/mr-review".into(),
                        canonical_path: Some("/home/u/.claude/skills/mr-review".into()),
                        root: ShadowRoot {
                            scope: ShadowRootScope::Global,
                            namespace: ShadowNamespace::Claude,
                            plugin: None,
                            path: "/home/u/.claude/skills".into(),
                            relation: ShadowRelation::CrossHarness,
                        },
                        appearances: vec![ShadowAppearance {
                            group: "g1".into(),
                            condition: "without_skill".into(),
                            eval_ids: vec!["e1".into()],
                            resolution: ShadowResolution::Selected,
                            precedence_rank: Some(1),
                        }],
                        remediation: Some(
                            "Set OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1 for every dispatch.".into(),
                        ),
                        verification: None,
                    }],
                    resolved_severity: None,
                }],
            },
            false,
        );

        let value = serde_json::to_value(artifact).unwrap();
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["findings"][0]["skill_name"], "mr-review");
        assert_eq!(
            value["findings"][0]["sources"][0]["root"]["relation"],
            "cross-harness"
        );
        assert!(value.get("shadowed").is_none());
    }

    #[test]
    fn observed_sources_merge_appearances_and_classify_symmetric_sibling_as_warning() {
        let root = ShadowRoot {
            scope: ShadowRootScope::Global,
            namespace: ShadowNamespace::Agents,
            plugin: None,
            path: "/home/u/.agents/skills".into(),
            relation: ShadowRelation::Native,
        };
        let mut with = ShadowSource::live_skill(
            "helper",
            Path::new("/home/u/.agents/skills/helper"),
            root.clone(),
            "Move the live helper.",
        );
        with.add_appearance(ShadowAppearance {
            group: "g1".into(),
            condition: "with_skill".into(),
            eval_ids: vec!["e1".into()],
            resolution: ShadowResolution::Coexisting,
            precedence_rank: None,
        });
        let mut without = with.clone();
        without.appearances.clear();
        without.add_appearance(ShadowAppearance {
            group: "g1".into(),
            condition: "without_skill".into(),
            eval_ids: vec!["e1".into()],
            resolution: ShadowResolution::Coexisting,
            precedence_rank: None,
        });

        let report = PluginShadowReport::from_observed_sources(
            "/x",
            vec![with, without],
            "subject",
            &[
                ("g1".into(), "with_skill".into()),
                ("g1".into(), "without_skill".into()),
            ],
        );

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].role, ShadowSkillRole::Sibling);
        assert_eq!(report.findings[0].severity, ShadowSeverity::Warning);
        assert_eq!(report.findings[0].sources.len(), 1);
        assert_eq!(report.findings[0].sources[0].appearances.len(), 2);
    }

    #[test]
    fn asymmetric_sibling_and_any_subject_collision_are_comparison_invalid() {
        let root = ShadowRoot::unknown("/live");
        let mut sibling = ShadowSource::live_skill(
            "helper",
            Path::new("/live/helper"),
            root.clone(),
            "Move it.",
        );
        sibling.add_appearance(ShadowAppearance {
            group: "g1".into(),
            condition: "with_skill".into(),
            eval_ids: vec!["e1".into()],
            resolution: ShadowResolution::Selected,
            precedence_rank: None,
        });
        let mut subject =
            ShadowSource::live_skill("subject", Path::new("/live/subject"), root, "Move it.");
        subject.add_appearance(ShadowAppearance {
            group: "g1".into(),
            condition: "with_skill".into(),
            eval_ids: vec!["e1".into()],
            resolution: ShadowResolution::Selected,
            precedence_rank: None,
        });

        let report = PluginShadowReport::from_observed_sources(
            "/x",
            vec![sibling, subject],
            "subject",
            &[
                ("g1".into(), "with_skill".into()),
                ("g1".into(), "without_skill".into()),
            ],
        );

        assert!(
            report
                .findings
                .iter()
                .all(|finding| { finding.severity == ShadowSeverity::ComparisonInvalid })
        );
    }

    #[test]
    fn shared_resolution_policies_record_precedence_and_coexistence() {
        let appearance = ShadowAppearance {
            group: "g1".into(),
            condition: "with_skill".into(),
            eval_ids: vec!["e1".into()],
            resolution: ShadowResolution::Unknown,
            precedence_rank: None,
        };
        let mut global = ShadowSource::live_skill(
            "helper",
            Path::new("/global/helper"),
            ShadowRoot {
                scope: ShadowRootScope::Global,
                namespace: ShadowNamespace::Claude,
                plugin: None,
                path: "/global".into(),
                relation: ShadowRelation::Native,
            },
            "Move it.",
        );
        global.add_appearance(appearance.clone());
        let mut project = ShadowSource::staged(
            "helper",
            "helper",
            Path::new("/project/helper"),
            ShadowRoot {
                scope: ShadowRootScope::Project,
                namespace: ShadowNamespace::Claude,
                plugin: None,
                path: "/project".into(),
                relation: ShadowRelation::Native,
            },
        );
        project.add_appearance(appearance);

        let mut precedence_sources = vec![global.clone(), project.clone()];
        resolve_by_precedence(&mut precedence_sources);
        assert_eq!(
            precedence_sources[0].appearances[0].resolution,
            ShadowResolution::Selected
        );
        assert_eq!(
            precedence_sources[1].appearances[0].resolution,
            ShadowResolution::Shadowed
        );
        assert_eq!(
            precedence_sources[0].appearances[0].precedence_rank,
            Some(1)
        );

        let mut coexisting_sources = vec![global, project];
        resolve_as_coexisting(&mut coexisting_sources);
        assert!(coexisting_sources.iter().all(|source| {
            source.appearances[0].resolution == ShadowResolution::Coexisting
                && source.appearances[0].precedence_rank.is_none()
        }));
    }

    #[test]
    fn selected_path_resolution_matches_skill_directories_and_skill_files() {
        let appearance = ShadowAppearance {
            group: "g1".into(),
            condition: "with_skill".into(),
            eval_ids: vec!["e1".into()],
            resolution: ShadowResolution::Unknown,
            precedence_rank: None,
        };
        let mut agents = ShadowSource::live_skill(
            "helper",
            Path::new("/repo/.agents/skills/helper"),
            ShadowRoot::unknown("/repo/.agents/skills"),
            "Move it.",
        );
        agents.add_appearance(appearance.clone());
        let mut opencode = ShadowSource::live_skill(
            "helper",
            Path::new("/repo/.opencode/skills/helper"),
            ShadowRoot::unknown("/repo/.opencode/skills"),
            "Move it.",
        );
        opencode.add_appearance(appearance);
        let mut sources = vec![agents, opencode];

        resolve_from_selected_paths(
            &mut sources,
            &BTreeMap::from([(
                "helper".into(),
                "/repo/.opencode/skills/helper/SKILL.md".into(),
            )]),
        );

        assert_eq!(
            sources[0].appearances[0].resolution,
            ShadowResolution::Shadowed
        );
        assert_eq!(
            sources[1].appearances[0].resolution,
            ShadowResolution::Selected
        );
    }
}
