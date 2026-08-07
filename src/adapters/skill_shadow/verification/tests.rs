//! The reconciliation policy: how transcript evidence resolves a finding.

use super::*;
use crate::adapters::skill_shadow::{
    ShadowAppearance, ShadowNamespace, ShadowRelation, ShadowResolution, ShadowRoot,
    ShadowRootScope, ShadowSkillRole,
};

/// One dispatch's evidence, as the policy sees it.
struct Dispatch {
    reported: bool,
    skills: Vec<String>,
    plugins: Vec<String>,
}

impl Dispatch {
    /// Reported a surface listing exactly these runtime ids and plugin keys.
    fn saw(skills: &[&str], plugins: &[&str]) -> Self {
        Self {
            reported: true,
            skills: skills.iter().map(|s| (*s).to_string()).collect(),
            plugins: plugins.iter().map(|p| (*p).to_string()).collect(),
        }
    }

    /// Reported an empty surface: nothing live loaded.
    fn clean() -> Self {
        Self::saw(&[], &[])
    }

    /// Ran, but its transcript reported nothing knowable.
    fn silent() -> Self {
        Self {
            reported: false,
            skills: Vec::new(),
            plugins: Vec::new(),
        }
    }
}

impl DispatchEvidence for Dispatch {
    fn has_evidence(&self) -> bool {
        self.reported
    }
    fn advertises(&self, runtime_id: &str) -> bool {
        self.reported && self.skills.iter().any(|s| s == runtime_id)
    }
    fn loaded_plugin(&self, plugin_key: &str) -> bool {
        self.reported && self.plugins.iter().any(|p| p == plugin_key)
    }
}

/// Maps `(condition)` to the dispatches recorded for it. Eval ids are ignored:
/// every fixture here uses a single eval.
struct Index(BTreeMap<String, Vec<Dispatch>>);

impl Index {
    fn of(cells: &[(&str, Vec<Dispatch>)]) -> Self {
        Self(
            cells
                .iter()
                .map(|(condition, dispatches)| {
                    (
                        (*condition).to_string(),
                        dispatches.iter().map(clone_dispatch).collect(),
                    )
                })
                .collect(),
        )
    }
}

fn clone_dispatch(d: &Dispatch) -> Dispatch {
    Dispatch {
        reported: d.reported,
        skills: d.skills.clone(),
        plugins: d.plugins.clone(),
    }
}

impl EvidenceIndex for Index {
    fn dispatches_for<'a>(
        &'a self,
        _eval_ids: &[String],
        condition: &str,
    ) -> Vec<&'a dyn DispatchEvidence> {
        self.0
            .get(condition)
            .map(|dispatches| {
                dispatches
                    .iter()
                    .map(|d| d as &dyn DispatchEvidence)
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn appearance(group: &str, condition: &str) -> ShadowAppearance {
    ShadowAppearance {
        group: group.to_string(),
        condition: condition.to_string(),
        eval_ids: vec!["e1".to_string()],
        resolution: ShadowResolution::Selected,
        precedence_rank: None,
    }
}

fn live_plugin_source(skill: &str, cells: &[(&str, &str)]) -> ShadowSource {
    ShadowSource {
        kind: ShadowSourceKind::Plugin,
        origin: ShadowSourceOrigin::Live,
        skill_name: skill.to_string(),
        runtime_id: format!("slow-powers:{skill}"),
        plugin: Some("slow-powers@slowdini".to_string()),
        discovery_path: format!("/cache/slow-powers/0.5.2/skills/{skill}"),
        canonical_path: None,
        root: ShadowRoot {
            scope: ShadowRootScope::Global,
            namespace: ShadowNamespace::Plugin,
            plugin: Some("slow-powers@slowdini".to_string()),
            path: "/cache/slow-powers/0.5.2/skills".to_string(),
            relation: ShadowRelation::Native,
        },
        appearances: cells.iter().map(|(g, c)| appearance(g, c)).collect(),
        remediation: Some("Disable it.".to_string()),
        verification: None,
    }
}

fn live_global_skill(skill: &str, cells: &[(&str, &str)]) -> ShadowSource {
    ShadowSource {
        kind: ShadowSourceKind::Skill,
        origin: ShadowSourceOrigin::Live,
        skill_name: skill.to_string(),
        runtime_id: skill.to_string(),
        plugin: None,
        discovery_path: format!("/home/u/.claude/skills/{skill}"),
        canonical_path: None,
        root: ShadowRoot {
            scope: ShadowRootScope::Global,
            namespace: ShadowNamespace::Claude,
            plugin: None,
            path: "/home/u/.claude/skills".to_string(),
            relation: ShadowRelation::Native,
        },
        appearances: cells.iter().map(|(g, c)| appearance(g, c)).collect(),
        remediation: Some("Move it.".to_string()),
        verification: None,
    }
}

fn staged_source(skill: &str, dir_name: &str, cells: &[(&str, &str)]) -> ShadowSource {
    ShadowSource {
        kind: ShadowSourceKind::Skill,
        origin: ShadowSourceOrigin::Staged,
        skill_name: skill.to_string(),
        runtime_id: skill.to_string(),
        plugin: None,
        discovery_path: format!("/env/.claude/skills/{dir_name}"),
        canonical_path: None,
        root: ShadowRoot {
            scope: ShadowRootScope::Project,
            namespace: ShadowNamespace::Claude,
            plugin: None,
            path: "/env/.claude/skills".to_string(),
            relation: ShadowRelation::Native,
        },
        appearances: cells.iter().map(|(g, c)| appearance(g, c)).collect(),
        remediation: None,
        verification: None,
    }
}

fn finding(role: ShadowSkillRole, sources: Vec<ShadowSource>) -> ShadowFinding {
    ShadowFinding {
        skill_name: sources[0].skill_name.clone(),
        role,
        severity: match role {
            ShadowSkillRole::Subject => ShadowSeverity::ComparisonInvalid,
            ShadowSkillRole::Sibling => ShadowSeverity::Warning,
        },
        sources,
        resolved_severity: None,
    }
}

/// The usual one-group, two-condition matrix.
fn one_group() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    BTreeMap::from([("g1", BTreeSet::from(["with_skill", "without_skill"]))])
}

const BOTH_CELLS: [(&str, &str); 2] = [("g1", "with_skill"), ("g1", "without_skill")];

#[test]
fn a_plugin_absent_from_every_dispatch_refutes_the_subject_finding() {
    let mut f = finding(
        ShadowSkillRole::Subject,
        vec![live_plugin_source("hardening-plans", &BOTH_CELLS)],
    );
    let index = Index::of(&[
        ("with_skill", vec![Dispatch::clean(), Dispatch::clean()]),
        ("without_skill", vec![Dispatch::clean(), Dispatch::clean()]),
    ]);

    let resolved = verify_finding(&mut f, &index, &one_group());

    assert_eq!(resolved, ShadowResolvedSeverity::Isolated);
    assert_eq!(finding_status(&f), VerificationStatus::Refuted);
    // The intrinsic severity records what the collision would have meant, and
    // is never rewritten by evidence.
    assert_eq!(f.severity, ShadowSeverity::ComparisonInvalid);
}

#[test]
fn a_plugin_loaded_in_one_cell_confirms_and_keeps_comparison_invalid() {
    let mut f = finding(
        ShadowSkillRole::Subject,
        vec![live_plugin_source("hardening-plans", &BOTH_CELLS)],
    );
    let index = Index::of(&[
        (
            "with_skill",
            vec![Dispatch::saw(&[], &["slow-powers@slowdini"])],
        ),
        ("without_skill", vec![Dispatch::clean()]),
    ]);

    let resolved = verify_finding(&mut f, &index, &one_group());

    assert_eq!(resolved, ShadowResolvedSeverity::ComparisonInvalid);
    assert_eq!(finding_status(&f), VerificationStatus::Confirmed);
    assert_eq!(confirmed_cells(&f), ["g1/with_skill"]);
}

#[test]
fn a_plugin_skill_advertised_without_the_plugin_key_still_confirms() {
    // The runtime id corroborates when only the skill roster names it.
    let mut f = finding(
        ShadowSkillRole::Subject,
        vec![live_plugin_source("hardening-plans", &BOTH_CELLS)],
    );
    let index = Index::of(&[
        (
            "with_skill",
            vec![Dispatch::saw(&["slow-powers:hardening-plans"], &[])],
        ),
        ("without_skill", vec![Dispatch::clean()]),
    ]);

    verify_finding(&mut f, &index, &one_group());
    assert_eq!(finding_status(&f), VerificationStatus::Confirmed);
}

#[test]
fn a_missing_transcript_leaves_the_cell_unverified_not_refuted() {
    // The whole point: absence of evidence must never read as evidence of
    // isolation, or the tool silently drops a real contamination warning.
    let mut f = finding(
        ShadowSkillRole::Subject,
        vec![live_plugin_source("hardening-plans", &BOTH_CELLS)],
    );
    let index = Index::of(&[
        ("with_skill", vec![Dispatch::clean()]),
        ("without_skill", vec![Dispatch::clean(), Dispatch::silent()]),
    ]);

    let resolved = verify_finding(&mut f, &index, &one_group());

    assert_eq!(resolved, ShadowResolvedSeverity::ComparisonInvalid);
    assert_eq!(finding_status(&f), VerificationStatus::Unverified);
    assert!(
        inconclusive_reason(&f)
            .unwrap()
            .contains("reported no skill/plugin surface")
    );
}

#[test]
fn a_cell_with_no_recorded_dispatch_is_unverified() {
    let mut f = finding(
        ShadowSkillRole::Subject,
        vec![live_plugin_source("hardening-plans", &BOTH_CELLS)],
    );
    // `without_skill` never made it into the surface report at all.
    let index = Index::of(&[("with_skill", vec![Dispatch::clean()])]);

    verify_finding(&mut f, &index, &one_group());

    assert_eq!(finding_status(&f), VerificationStatus::Unverified);
    assert!(
        inconclusive_reason(&f)
            .unwrap()
            .contains("no dispatch was recorded")
    );
}

#[test]
fn a_confirmed_cell_outranks_an_unverified_one() {
    // Proven contamination in one cell beats a reporting gap in another: the
    // finding is real regardless of what the silent dispatch would have shown.
    let mut f = finding(
        ShadowSkillRole::Subject,
        vec![live_plugin_source("hardening-plans", &BOTH_CELLS)],
    );
    let index = Index::of(&[
        (
            "with_skill",
            vec![Dispatch::saw(&[], &["slow-powers@slowdini"])],
        ),
        ("without_skill", vec![Dispatch::silent()]),
    ]);

    let resolved = verify_finding(&mut f, &index, &one_group());

    assert_eq!(resolved, ShadowResolvedSeverity::ComparisonInvalid);
    assert_eq!(finding_status(&f), VerificationStatus::Confirmed);
}

#[test]
fn a_live_skill_colliding_with_a_staged_runtime_id_is_inconclusive() {
    // A staged sibling advertises the same string as the live global copy, so a
    // transcript cannot tell which one the agent saw.
    let mut f = finding(
        ShadowSkillRole::Sibling,
        vec![
            live_global_skill("writing-skills", &BOTH_CELLS),
            staged_source("writing-skills", "writing-skills", &BOTH_CELLS),
        ],
    );
    let index = Index::of(&[
        ("with_skill", vec![Dispatch::saw(&["writing-skills"], &[])]),
        (
            "without_skill",
            vec![Dispatch::saw(&["writing-skills"], &[])],
        ),
    ]);

    verify_finding(&mut f, &index, &one_group());

    assert_eq!(finding_status(&f), VerificationStatus::Unverified);
    assert!(
        inconclusive_reason(&f)
            .unwrap()
            .contains("indistinguishable")
    );
}

#[test]
fn a_live_skill_colliding_with_a_staged_directory_name_is_inconclusive() {
    // Belt-and-braces: the staged source's recorded runtime_id may be the logical
    // name while the harness advertises the staged *directory*. Either match has
    // to block a verdict.
    let mut staged = staged_source("hardening-plans", "eval-2__hardening-plans", &BOTH_CELLS);
    staged.runtime_id = "hardening-plans".to_string();
    let mut live = live_global_skill("hardening-plans", &BOTH_CELLS);
    live.runtime_id = "eval-2__hardening-plans".to_string();

    let mut f = finding(ShadowSkillRole::Subject, vec![live, staged]);
    let index = Index::of(&[
        ("with_skill", vec![Dispatch::clean()]),
        ("without_skill", vec![Dispatch::clean()]),
    ]);

    verify_finding(&mut f, &index, &one_group());

    assert_eq!(
        finding_status(&f),
        VerificationStatus::Unverified,
        "a staged directory sharing the live runtime id must block a refutation"
    );
}

#[test]
fn evidence_downgrades_an_asymmetric_sibling_when_both_arms_actually_loaded_it() {
    // Detection saw the source expected in only one arm and called it
    // comparison-invalid. Evidence shows both arms loaded it, so the
    // contamination is symmetric and the delta stays comparable.
    let mut f = finding(
        ShadowSkillRole::Sibling,
        vec![live_plugin_source(
            "writing-skills",
            &[("g1", "with_skill")],
        )],
    );
    f.severity = ShadowSeverity::ComparisonInvalid;
    f.sources[0]
        .appearances
        .push(appearance("g1", "without_skill"));
    let index = Index::of(&[
        (
            "with_skill",
            vec![Dispatch::saw(&[], &["slow-powers@slowdini"])],
        ),
        (
            "without_skill",
            vec![Dispatch::saw(&[], &["slow-powers@slowdini"])],
        ),
    ]);

    let resolved = verify_finding(&mut f, &index, &one_group());

    assert_eq!(resolved, ShadowResolvedSeverity::Warning);
    assert_eq!(f.severity, ShadowSeverity::ComparisonInvalid, "intrinsic");
}

#[test]
fn evidence_upgrades_a_symmetric_sibling_when_only_one_arm_loaded_it() {
    let mut f = finding(
        ShadowSkillRole::Sibling,
        vec![live_plugin_source("writing-skills", &BOTH_CELLS)],
    );
    let index = Index::of(&[
        (
            "with_skill",
            vec![Dispatch::saw(&[], &["slow-powers@slowdini"])],
        ),
        ("without_skill", vec![Dispatch::clean()]),
    ]);

    let resolved = verify_finding(&mut f, &index, &one_group());

    assert_eq!(resolved, ShadowResolvedSeverity::ComparisonInvalid);
    assert_eq!(
        f.severity,
        ShadowSeverity::Warning,
        "intrinsic is untouched"
    );
}

#[test]
fn a_finding_with_only_staged_sources_stays_at_its_intrinsic_severity() {
    let mut f = finding(
        ShadowSkillRole::Subject,
        vec![staged_source(
            "hardening-plans",
            "hardening-plans",
            &BOTH_CELLS,
        )],
    );
    let index = Index::of(&[("with_skill", vec![Dispatch::clean()])]);

    let resolved = verify_finding(&mut f, &index, &one_group());

    assert_eq!(resolved, ShadowResolvedSeverity::ComparisonInvalid);
    assert_eq!(finding_status(&f), VerificationStatus::Unverified);
}

#[test]
fn every_live_source_must_be_refuted_to_isolate_the_finding() {
    // Two cached plugin versions: refuting one while the other is unverified
    // leaves the finding unresolved.
    let mut second = live_plugin_source("hardening-plans", &BOTH_CELLS);
    second.discovery_path = "/cache/slow-powers/0.5.4/skills/hardening-plans".to_string();
    second.plugin = Some("other@marketplace".to_string());
    let mut f = finding(
        ShadowSkillRole::Subject,
        vec![live_plugin_source("hardening-plans", &BOTH_CELLS), second],
    );
    let index = Index::of(&[
        ("with_skill", vec![Dispatch::clean()]),
        ("without_skill", vec![Dispatch::silent()]),
    ]);

    let resolved = verify_finding(&mut f, &index, &one_group());

    assert_ne!(resolved, ShadowResolvedSeverity::Isolated);
    assert_eq!(finding_status(&f), VerificationStatus::Unverified);
}

#[test]
fn confirming_dispatch_count_totals_the_reporting_dispatches() {
    let mut f = finding(
        ShadowSkillRole::Subject,
        vec![live_plugin_source("hardening-plans", &BOTH_CELLS)],
    );
    let index = Index::of(&[
        (
            "with_skill",
            vec![
                Dispatch::saw(&[], &["slow-powers@slowdini"]),
                Dispatch::saw(&[], &["slow-powers@slowdini"]),
            ],
        ),
        (
            "without_skill",
            vec![Dispatch::saw(&[], &["slow-powers@slowdini"])],
        ),
    ]);

    verify_finding(&mut f, &index, &one_group());

    assert_eq!(confirming_dispatch_count(&f), 3);
}
