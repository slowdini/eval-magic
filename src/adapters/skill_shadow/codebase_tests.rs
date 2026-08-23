use super::*;

fn sample_report() -> PluginShadowReport {
    PluginShadowReport::from_sources(
        "/x",
        vec![ShadowSource {
            kind: ShadowSourceKind::Plugin,
            origin: ShadowSourceOrigin::Live,
            skill_name: "subject".into(),
            runtime_id: "plugin:subject".into(),
            plugin: Some("plugin@example".into()),
            discovery_path: "/plugins/example/subject".into(),
            canonical_path: None,
            root: ShadowRoot::unknown("/plugins/example"),
            appearances: Vec::new(),
            remediation: None,
            verification: None,
        }],
    )
}

#[test]
fn validity_warnings_can_be_filtered_by_finding_class() {
    let mut report = sample_report();
    report.findings[0].class = ShadowFindingClass::CodebaseSourced;
    let artifact = PluginShadowArtifact::new(report, true);

    assert!(
        artifact
            .validity_warnings_for_class(ShadowFindingClass::OperatorEnvironment)
            .is_empty()
    );
    assert_eq!(
        artifact
            .validity_warnings_for_class(ShadowFindingClass::CodebaseSourced)
            .len(),
        1
    );
}

#[test]
fn codebase_finding_banner_names_the_sourced_codebase_not_operator_environment() {
    let mut report = sample_report();
    report.findings[0].class = ShadowFindingClass::CodebaseSourced;

    let banner = format_shadow_banner(&report);

    assert!(banner.contains("sourced codebase"), "{banner}");
    assert!(!banner.contains("operator environment"), "{banner}");
}

#[test]
fn v2_artifact_defaults_findings_to_the_operator_environment_class() {
    let artifact: PluginShadowArtifact = serde_json::from_value(serde_json::json!({
        "schema_version": 2,
        "config_dir": "/home/u/.claude",
        "findings": [{
            "skill_name": "mr-review",
            "role": "subject",
            "severity": "comparison-invalid",
            "sources": []
        }]
    }))
    .unwrap();

    assert_eq!(
        artifact.report.findings[0].class,
        ShadowFindingClass::OperatorEnvironment
    );
    assert_eq!(
        serde_json::to_value(artifact).unwrap()["schema_version"],
        PLUGIN_SHADOW_SCHEMA_VERSION
    );
}

#[test]
fn observed_codebase_sources_keep_their_distinct_finding_class() {
    let source = ShadowSource::live_skill(
        "subject",
        Path::new("/repo/.claude/skills/subject"),
        ShadowRoot {
            scope: ShadowRootScope::Project,
            namespace: ShadowNamespace::Claude,
            plugin: None,
            path: "/repo/.claude/skills".into(),
            relation: ShadowRelation::Native,
        },
        "Set `codebase.exclude_skill_sources = true` for this eval.",
    );

    let report = PluginShadowReport::from_observed_sources_with_class(
        "/repo",
        vec![source],
        "subject",
        &[("g1".into(), "with_skill".into())],
        ShadowFindingClass::CodebaseSourced,
    );

    assert_eq!(report.findings.len(), 1);
    assert_eq!(
        report.findings[0].class,
        ShadowFindingClass::CodebaseSourced
    );
}
