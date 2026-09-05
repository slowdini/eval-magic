//! Joining the shadow findings to the session-surface evidence.
//!
//! Thin I/O around the policy in
//! [`skill_shadow::verification`](crate::adapters::skill_shadow::verification):
//! read both artifacts, resolve every finding, write the verdict back into
//! `plugin-shadow.json`. Doing it here rather than in `aggregate` means the
//! verdict is persisted provenance, not something recomputed per read.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::adapters::skill_shadow::verification::{
    DispatchEvidence, EvidenceIndex, ReportVerification, VerificationStatus, finding_status,
    verify_finding,
};
use crate::adapters::skill_shadow::{PluginShadowArtifact, ShadowFindingClass};
use crate::core::fs::write_json;
use crate::pipeline::error::PipelineError;
use crate::pipeline::io::now_iso8601;
use crate::pipeline::session_surface::{SessionSurfaceReport, TaskSessionSurface};
use crate::validation::{SchemaName, validate_against_schema};

impl DispatchEvidence for TaskSessionSurface {
    fn has_evidence(&self) -> bool {
        TaskSessionSurface::has_evidence(self)
    }
    fn advertises(&self, runtime_id: &str) -> bool {
        TaskSessionSurface::advertises(self, runtime_id)
    }
    fn loaded_plugin(&self, plugin_key: &str) -> bool {
        TaskSessionSurface::loaded_plugin(self, plugin_key)
    }
}

struct SurfaceIndex<'a>(&'a SessionSurfaceReport);

impl EvidenceIndex for SurfaceIndex<'_> {
    fn dispatches_for<'a>(
        &'a self,
        eval_ids: &[String],
        condition: &str,
    ) -> Vec<&'a dyn DispatchEvidence> {
        eval_ids
            .iter()
            .flat_map(|eval_id| self.0.tasks_for(eval_id, condition))
            .map(|task| task as &dyn DispatchEvidence)
            .collect()
    }
}

/// Resolve every finding in `plugin-shadow.json` against `session-surface.json`
/// and write the result back.
///
/// A no-op when either artifact is missing: without evidence there is nothing to
/// resolve, and an unwritten verdict is exactly how "never verified" is encoded.
/// Legacy unversioned reports carry no per-cell appearances, so they are left
/// alone too.
pub(crate) fn verify_iteration(iteration_dir: &Path) -> Result<(), PipelineError> {
    let shadow_path = iteration_dir.join("plugin-shadow.json");
    let Ok(raw) = fs::read_to_string(&shadow_path) else {
        return Ok(());
    };
    let Ok(mut artifact) = serde_json::from_str::<PluginShadowArtifact>(&raw) else {
        return Ok(());
    };
    if artifact.is_legacy() {
        return Ok(());
    }
    let Ok(surface_raw) = fs::read_to_string(iteration_dir.join("session-surface.json")) else {
        return Ok(());
    };
    let Ok(surfaces) = serde_json::from_str::<SessionSurfaceReport>(&surface_raw) else {
        return Ok(());
    };

    // The cell matrix, rebuilt from the findings' own appearances: the sibling
    // symmetry rule needs to know every condition of every group.
    let mut expected: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for finding in &artifact.report.findings {
        for source in &finding.sources {
            for appearance in &source.appearances {
                expected
                    .entry(appearance.group.clone())
                    .or_default()
                    .insert(appearance.condition.clone());
            }
        }
    }
    let expected_by_group: BTreeMap<&str, BTreeSet<&str>> = expected
        .iter()
        .map(|(group, conditions)| {
            (
                group.as_str(),
                conditions.iter().map(String::as_str).collect(),
            )
        })
        .collect();

    let index = SurfaceIndex(&surfaces);
    let (mut refuted, mut confirmed, mut unverified, mut confirmed_operator) = (0, 0, 0, 0);
    for finding in &mut artifact.report.findings {
        finding.resolved_severity = Some(verify_finding(finding, &index, &expected_by_group));
        match finding_status(finding) {
            VerificationStatus::Refuted => refuted += 1,
            VerificationStatus::Confirmed => {
                confirmed += 1;
                if finding.class == ShadowFindingClass::OperatorEnvironment {
                    confirmed_operator += 1;
                }
            }
            VerificationStatus::Unverified => unverified += 1,
        }
    }

    artifact.verification = Some(ReportVerification {
        generated: now_iso8601(),
        harness_reports_session_surface: true,
        dispatches_with_evidence: surfaces.tasks_with_evidence,
        dispatches_without_evidence: surfaces.tasks_without_evidence,
        refuted_findings: refuted,
        confirmed_findings: confirmed,
        unverified_findings: unverified,
        // A declared assertion that evidence contradicts: the suppressed
        // findings were real, which is worth saying louder than the assertion.
        assertion_contradicted: artifact.isolates_live_sources && confirmed_operator > 0,
    });

    write_verified(&shadow_path, &artifact)
}

/// Write the artifact through the schema gate. The preflight's own write goes
/// through the same gate, so the two cannot drift.
pub(crate) fn write_verified(
    path: &Path,
    artifact: &PluginShadowArtifact,
) -> Result<(), PipelineError> {
    validate_against_schema::<serde_json::Value>(
        SchemaName::PluginShadow,
        &serde_json::to_value(artifact)?,
        &path.to_string_lossy(),
    )?;
    write_json(path, artifact)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::skill_shadow::{
        PluginShadowReport, ShadowAppearance, ShadowFinding, ShadowFindingClass, ShadowNamespace,
        ShadowRelation, ShadowResolution, ShadowResolvedSeverity, ShadowRoot, ShadowRootScope,
        ShadowSeverity, ShadowSkillRole, ShadowSource, ShadowSourceKind, ShadowSourceOrigin,
    };
    use crate::adapters::{LoadedPlugin, SessionSurface};
    use crate::pipeline::session_surface::RoundSurface;
    use tempfile::TempDir;

    fn shadow_artifact(isolates: bool) -> PluginShadowArtifact {
        let source = ShadowSource {
            kind: ShadowSourceKind::Plugin,
            origin: ShadowSourceOrigin::Live,
            skill_name: "mr-review".into(),
            runtime_id: "slow-powers:mr-review".into(),
            plugin: Some("slow-powers@slowdini".into()),
            discovery_path: "/cache/slow-powers/0.5.2/skills/mr-review".into(),
            canonical_path: None,
            root: ShadowRoot {
                scope: ShadowRootScope::Global,
                namespace: ShadowNamespace::Plugin,
                plugin: Some("slow-powers@slowdini".into()),
                path: "/cache/slow-powers/0.5.2/skills".into(),
                relation: ShadowRelation::Native,
            },
            appearances: ["with_skill", "without_skill"]
                .iter()
                .map(|condition| ShadowAppearance {
                    group: "g1".into(),
                    condition: (*condition).into(),
                    eval_ids: vec!["e1".into()],
                    resolution: ShadowResolution::Selected,
                    precedence_rank: None,
                })
                .collect(),
            remediation: Some("Disable it.".into()),
            verification: None,
        };
        PluginShadowArtifact::new(
            PluginShadowReport {
                config_dir: "/home/u/.claude".into(),
                findings: vec![ShadowFinding {
                    class: ShadowFindingClass::OperatorEnvironment,
                    skill_name: "mr-review".into(),
                    role: ShadowSkillRole::Subject,
                    severity: ShadowSeverity::ComparisonInvalid,
                    sources: vec![source],
                    resolved_severity: None,
                }],
            },
            isolates,
        )
    }

    fn surface_report(tasks: Vec<TaskSessionSurface>) -> SessionSurfaceReport {
        let with = tasks.iter().filter(|t| t.has_evidence()).count();
        SessionSurfaceReport {
            generated: "2026-08-07T00:00:00Z".into(),
            iteration: 1,
            tasks_with_evidence: with,
            tasks_without_evidence: tasks.len() - with,
            tasks,
        }
    }

    fn task(condition: &str, plugins: &[&str], reported: bool) -> TaskSessionSurface {
        TaskSessionSurface {
            eval_id: "e1".into(),
            condition: condition.into(),
            run_index: None,
            group: Some("g1".into()),
            rounds: vec![RoundSurface {
                round: 1,
                surface: reported.then(|| SessionSurface {
                    advertised_skills: Vec::new(),
                    loaded_plugins: plugins
                        .iter()
                        .map(|key| LoadedPlugin {
                            name: key.split('@').next().unwrap().to_string(),
                            source: Some((*key).to_string()),
                            version: None,
                        })
                        .collect(),
                }),
            }],
        }
    }

    fn write_both(dir: &Path, artifact: &PluginShadowArtifact, surfaces: &SessionSurfaceReport) {
        write_json(&dir.join("plugin-shadow.json"), artifact).unwrap();
        write_json(&dir.join("session-surface.json"), surfaces).unwrap();
    }

    fn read_back(dir: &Path) -> PluginShadowArtifact {
        serde_json::from_str(&fs::read_to_string(dir.join("plugin-shadow.json")).unwrap()).unwrap()
    }

    #[test]
    fn an_isolated_run_resolves_every_finding_to_isolated() {
        let dir = TempDir::new().unwrap();
        write_both(
            dir.path(),
            &shadow_artifact(false),
            &surface_report(vec![
                task("with_skill", &[], true),
                task("without_skill", &[], true),
            ]),
        );

        verify_iteration(dir.path()).unwrap();

        let artifact = read_back(dir.path());
        assert_eq!(
            artifact.report.findings[0].resolved_severity,
            Some(ShadowResolvedSeverity::Isolated)
        );
        let verification = artifact.verification.unwrap();
        assert_eq!(verification.refuted_findings, 1);
        assert_eq!(verification.confirmed_findings, 0);
        assert!(!verification.assertion_contradicted);
    }

    #[test]
    fn a_loaded_plugin_keeps_the_finding_comparison_invalid() {
        let dir = TempDir::new().unwrap();
        write_both(
            dir.path(),
            &shadow_artifact(false),
            &surface_report(vec![
                task("with_skill", &["slow-powers@slowdini"], true),
                task("without_skill", &["slow-powers@slowdini"], true),
            ]),
        );

        verify_iteration(dir.path()).unwrap();

        let artifact = read_back(dir.path());
        assert_eq!(
            artifact.report.findings[0].resolved_severity,
            Some(ShadowResolvedSeverity::ComparisonInvalid)
        );
        assert_eq!(artifact.verification.unwrap().confirmed_findings, 1);
    }

    #[test]
    fn a_declared_assertion_contradicted_by_evidence_is_flagged() {
        let dir = TempDir::new().unwrap();
        write_both(
            dir.path(),
            &shadow_artifact(true),
            &surface_report(vec![
                task("with_skill", &["slow-powers@slowdini"], true),
                task("without_skill", &[], true),
            ]),
        );

        verify_iteration(dir.path()).unwrap();

        assert!(
            read_back(dir.path())
                .verification
                .unwrap()
                .assertion_contradicted,
            "a false isolation assertion must be reported, not trusted"
        );
    }

    #[test]
    fn a_loaded_codebase_source_does_not_contradict_operator_isolation() {
        let dir = TempDir::new().unwrap();
        let mut artifact = shadow_artifact(true);
        artifact.report.findings[0].class = ShadowFindingClass::CodebaseSourced;
        write_both(
            dir.path(),
            &artifact,
            &surface_report(vec![
                task("with_skill", &["slow-powers@slowdini"], true),
                task("without_skill", &[], true),
            ]),
        );

        verify_iteration(dir.path()).unwrap();

        let artifact = read_back(dir.path());
        assert_eq!(artifact.verification.unwrap().confirmed_findings, 1);
        assert!(
            !read_back(dir.path())
                .verification
                .unwrap()
                .assertion_contradicted,
            "operator isolation does not make claims about the sourced codebase"
        );
    }

    #[test]
    fn a_missing_surface_report_leaves_the_artifact_untouched() {
        let dir = TempDir::new().unwrap();
        write_json(
            &dir.path().join("plugin-shadow.json"),
            &shadow_artifact(false),
        )
        .unwrap();

        verify_iteration(dir.path()).unwrap();

        let artifact = read_back(dir.path());
        assert_eq!(artifact.report.findings[0].resolved_severity, None);
        assert!(artifact.verification.is_none());
    }

    #[test]
    fn a_legacy_unversioned_artifact_is_left_unverified() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("plugin-shadow.json"),
            serde_json::to_string(&serde_json::json!({
                "config_dir": "/home/u/.claude",
                "shadowed": [{"kind": "plugin", "plugin": "p@m", "skill_name": "mr-review",
                              "path": "/cache/p/skills/mr-review"}],
            }))
            .unwrap(),
        )
        .unwrap();
        write_json(
            &dir.path().join("session-surface.json"),
            &surface_report(vec![task("with_skill", &[], true)]),
        )
        .unwrap();

        verify_iteration(dir.path()).unwrap();

        // Untouched: it has no appearances to join evidence against.
        let artifact = read_back(dir.path());
        assert!(artifact.verification.is_none());
        assert_eq!(artifact.report.findings[0].resolved_severity, None);
    }

    #[test]
    fn a_silent_dispatch_leaves_the_finding_unverified() {
        let dir = TempDir::new().unwrap();
        write_both(
            dir.path(),
            &shadow_artifact(false),
            &surface_report(vec![
                task("with_skill", &[], true),
                task("without_skill", &[], false),
            ]),
        );

        verify_iteration(dir.path()).unwrap();

        let artifact = read_back(dir.path());
        assert_eq!(
            artifact.report.findings[0].resolved_severity,
            Some(ShadowResolvedSeverity::ComparisonInvalid),
            "an unverified subject collision keeps its intrinsic severity"
        );
        assert_eq!(artifact.verification.unwrap().unverified_findings, 1);
    }
}
