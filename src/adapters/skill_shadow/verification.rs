//! Reconciling build-time shadow findings against what dispatches actually
//! loaded.
//!
//! Detection runs before anything dispatches, so it can only report what is
//! *discoverable* from an environment. That makes `comparison invalid` a
//! statement about risk, not about the run — and on a correctly-isolated
//! campaign it is simply wrong (issue #207). This module applies the evidence
//! each dispatch's own transcript carries and resolves every finding to one of
//! three states.
//!
//! The asymmetry between them is deliberate. Confirming needs one dispatch that
//! saw the source; refuting needs *every* expected cell to have reported, and
//! reported nothing. A missing transcript is never a refutation — the tool would
//! rather keep a warning an operator can dismiss than drop one that was real.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    ShadowFinding, ShadowSeverity, ShadowSource, ShadowSourceKind, ShadowSourceOrigin, severity_for,
};

/// What a dispatch's transcript says about one live source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerificationStatus {
    /// A dispatch reported the source as discoverable.
    Confirmed,
    /// Every expected cell reported, and none saw it.
    Refuted,
    /// At least one expected cell reported nothing usable.
    Unverified,
}

/// A finding's severity after evidence is applied. Distinct from the intrinsic
/// [`ShadowSeverity`], which records what the collision would mean if it loaded
/// and is never rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowResolvedSeverity {
    /// Every live source was refuted: the dispatches were isolated.
    Isolated,
    Warning,
    ComparisonInvalid,
}

/// One comparison cell's verdict for one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellVerification {
    pub group: String,
    pub condition: String,
    pub status: VerificationStatus,
    pub dispatches_with_evidence: usize,
    pub dispatches_without_evidence: usize,
    /// Why presence and absence could not be told apart here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inconclusive_reason: Option<String>,
}

/// One live source's verdict across every cell it was expected in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceVerification {
    pub status: VerificationStatus,
    pub cells: Vec<CellVerification>,
}

/// Run-level totals, written alongside the findings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportVerification {
    pub generated: String,
    /// False when the harness's transcripts carry no skill/plugin roster, which
    /// is why everything stayed unverified.
    pub harness_reports_session_surface: bool,
    pub dispatches_with_evidence: usize,
    pub dispatches_without_evidence: usize,
    pub refuted_findings: usize,
    pub confirmed_findings: usize,
    pub unverified_findings: usize,
    /// A descriptor declared `isolates_live_sources = true` while evidence shows
    /// a source loaded anyway.
    pub assertion_contradicted: bool,
}

/// What one dispatch reported, as the reconciler needs it. Implemented by the
/// pipeline's per-task record; kept as a trait so the policy stays free of the
/// artifact types and testable on its own.
pub(crate) trait DispatchEvidence {
    /// `false` when any round reported nothing knowable.
    fn has_evidence(&self) -> bool;
    fn advertises(&self, runtime_id: &str) -> bool;
    fn loaded_plugin(&self, plugin_key: &str) -> bool;
}

/// Whether `dispatch` shows `source` was discoverable.
///
/// For a plugin source the plugin key is decisive: a loaded plugin makes
/// everything it ships discoverable, even if its skill set changed between the
/// scan and the dispatch. The advertised runtime id corroborates. For a direct
/// skill source only the runtime id is available.
fn shows(source: &ShadowSource, dispatch: &dyn DispatchEvidence) -> bool {
    match source.kind {
        ShadowSourceKind::Plugin => {
            source
                .plugin
                .as_deref()
                .is_some_and(|key| dispatch.loaded_plugin(key))
                || dispatch.advertises(&source.runtime_id)
        }
        ShadowSourceKind::Skill => dispatch.advertises(&source.runtime_id),
    }
}

/// A staged source in the same finding that would advertise the same runtime id
/// as `live`, making the two indistinguishable in a transcript.
///
/// Compares the recorded `runtime_id` and, belt-and-braces, the staged
/// directory name: harnesses that advertise a staged skill by its directory
/// rather than its logical name would otherwise slip past.
///
/// The basename is split on either separator. `discovery_path` is wire format
/// (forward slashes) while the host separator may be `\`, so keying off
/// `MAIN_SEPARATOR` would find no basename at all and let a refutation through
/// on the very collision this exists to block.
fn colliding_staged_source<'a>(
    finding: &'a ShadowFinding,
    live: &ShadowSource,
    group: &str,
    condition: &str,
) -> Option<&'a ShadowSource> {
    finding
        .sources
        .iter()
        .filter(|source| source.origin == ShadowSourceOrigin::Staged)
        .filter(|source| {
            source
                .appearances
                .iter()
                .any(|appearance| appearance.group == group && appearance.condition == condition)
        })
        .find(|staged| {
            staged.runtime_id == live.runtime_id
                || staged
                    .discovery_path
                    .rsplit(['/', '\\'])
                    .next()
                    .is_some_and(|basename| basename == live.runtime_id)
        })
}

/// The cells one live source was expected to be discoverable in.
fn expected_cells(source: &ShadowSource) -> BTreeSet<(String, String)> {
    source
        .appearances
        .iter()
        .map(|appearance| (appearance.group.clone(), appearance.condition.clone()))
        .collect()
}

/// Resolve one cell, given every dispatch recorded for it.
///
/// Precedence is Confirmed > Unverified > Refuted, and the order matters:
/// contamination proven in one dispatch outranks a reporting gap in another,
/// because the finding is real either way.
fn verify_cell(
    finding: &ShadowFinding,
    source: &ShadowSource,
    group: &str,
    condition: &str,
    dispatches: &[&dyn DispatchEvidence],
) -> CellVerification {
    let with_evidence = dispatches.iter().filter(|d| d.has_evidence()).count();
    let mut cell = CellVerification {
        group: group.to_string(),
        condition: condition.to_string(),
        status: VerificationStatus::Unverified,
        dispatches_with_evidence: with_evidence,
        dispatches_without_evidence: dispatches.len() - with_evidence,
        inconclusive_reason: None,
    };

    // The collision guard gates *both* verdicts, so it runs first. When a staged
    // copy advertises the same runtime id, seeing that id proves nothing (the
    // agent may have resolved the staged one) and not seeing it proves nothing
    // either. Only an unambiguous id can confirm or refute.
    if let Some(staged) = colliding_staged_source(finding, source, group, condition) {
        cell.inconclusive_reason = Some(format!(
            "runtime id '{}' is also staged here (as '{}'), so presence is indistinguishable",
            source.runtime_id, staged.discovery_path
        ));
        return cell;
    }
    if dispatches.iter().any(|d| shows(source, *d)) {
        cell.status = VerificationStatus::Confirmed;
        return cell;
    }
    if dispatches.is_empty() || with_evidence < dispatches.len() {
        cell.inconclusive_reason = Some(if dispatches.is_empty() {
            "no dispatch was recorded for this cell".to_string()
        } else {
            format!(
                "{} of {} dispatch(es) reported no skill/plugin surface",
                dispatches.len() - with_evidence,
                dispatches.len()
            )
        });
        return cell;
    }
    cell.status = VerificationStatus::Refuted;
    cell
}

/// Roll a source's cells up to one status: any confirmation wins, otherwise every
/// cell must be refuted to refute the source.
fn roll_up(cells: &[CellVerification]) -> VerificationStatus {
    if cells
        .iter()
        .any(|cell| cell.status == VerificationStatus::Confirmed)
    {
        VerificationStatus::Confirmed
    } else if !cells.is_empty()
        && cells
            .iter()
            .all(|cell| cell.status == VerificationStatus::Refuted)
    {
        VerificationStatus::Refuted
    } else {
        VerificationStatus::Unverified
    }
}

/// Look up the dispatches recorded for one cell.
pub(crate) trait EvidenceIndex {
    fn dispatches_for<'a>(
        &'a self,
        eval_ids: &[String],
        condition: &str,
    ) -> Vec<&'a dyn DispatchEvidence>;
}

/// Verify every live source of `finding` and resolve the finding's severity.
///
/// `expected_by_group` is the run's full cell matrix, needed to re-apply the
/// sibling-symmetry rule to the *confirmed* cells only. That can legitimately
/// move a verdict in either direction: a sibling that looked asymmetric because
/// only one arm's environment held the source is downgraded when evidence shows
/// both arms loaded it, and one that looked symmetric is upgraded when evidence
/// shows only one arm did.
pub(crate) fn verify_finding(
    finding: &mut ShadowFinding,
    index: &dyn EvidenceIndex,
    expected_by_group: &BTreeMap<&str, BTreeSet<&str>>,
) -> ShadowResolvedSeverity {
    let live_indices: Vec<usize> = finding
        .sources
        .iter()
        .enumerate()
        .filter(|(_, source)| source.origin == ShadowSourceOrigin::Live)
        .map(|(index, _)| index)
        .collect();

    let mut verifications: Vec<(usize, SourceVerification)> = Vec::new();
    for &position in &live_indices {
        let source = &finding.sources[position];
        let mut cells: Vec<CellVerification> = Vec::new();
        for (group, condition) in expected_cells(source) {
            let eval_ids: Vec<String> = source
                .appearances
                .iter()
                .filter(|a| a.group == group && a.condition == condition)
                .flat_map(|a| a.eval_ids.clone())
                .collect();
            let dispatches = index.dispatches_for(&eval_ids, &condition);
            cells.push(verify_cell(
                finding,
                source,
                &group,
                &condition,
                &dispatches,
            ));
        }
        let status = roll_up(&cells);
        verifications.push((position, SourceVerification { status, cells }));
    }

    let confirmed_owned: BTreeSet<(String, String)> = verifications
        .iter()
        .flat_map(|(_, verification)| &verification.cells)
        .filter(|cell| cell.status == VerificationStatus::Confirmed)
        .map(|cell| (cell.group.clone(), cell.condition.clone()))
        .collect();
    let statuses: Vec<VerificationStatus> = verifications
        .iter()
        .map(|(_, verification)| verification.status)
        .collect();

    for (position, verification) in verifications {
        finding.sources[position].verification = Some(verification);
    }
    let confirmed_cells: BTreeSet<(&str, &str)> = confirmed_owned
        .iter()
        .map(|(group, condition)| (group.as_str(), condition.as_str()))
        .collect();

    if statuses.is_empty() {
        return match finding.severity {
            ShadowSeverity::Warning => ShadowResolvedSeverity::Warning,
            ShadowSeverity::ComparisonInvalid => ShadowResolvedSeverity::ComparisonInvalid,
        };
    }
    if statuses.contains(&VerificationStatus::Confirmed) {
        return match severity_for(finding.role, &confirmed_cells, expected_by_group) {
            ShadowSeverity::Warning => ShadowResolvedSeverity::Warning,
            ShadowSeverity::ComparisonInvalid => ShadowResolvedSeverity::ComparisonInvalid,
        };
    }
    if statuses
        .iter()
        .all(|status| *status == VerificationStatus::Refuted)
    {
        return ShadowResolvedSeverity::Isolated;
    }
    match finding.severity {
        ShadowSeverity::Warning => ShadowResolvedSeverity::Warning,
        ShadowSeverity::ComparisonInvalid => ShadowResolvedSeverity::ComparisonInvalid,
    }
}

/// A finding's rolled-up status, for reporting.
pub(crate) fn finding_status(finding: &ShadowFinding) -> VerificationStatus {
    let statuses: Vec<VerificationStatus> = finding
        .sources
        .iter()
        .filter(|source| source.origin == ShadowSourceOrigin::Live)
        .filter_map(|source| source.verification.as_ref())
        .map(|verification| verification.status)
        .collect();
    if statuses.is_empty() {
        return VerificationStatus::Unverified;
    }
    if statuses.contains(&VerificationStatus::Confirmed) {
        VerificationStatus::Confirmed
    } else if statuses
        .iter()
        .all(|status| *status == VerificationStatus::Refuted)
    {
        VerificationStatus::Refuted
    } else {
        VerificationStatus::Unverified
    }
}

/// The cells a confirmed source was actually seen in, for the warning text.
pub(crate) fn confirmed_cells(finding: &ShadowFinding) -> Vec<String> {
    let mut cells: Vec<String> = finding
        .sources
        .iter()
        .filter_map(|source| source.verification.as_ref())
        .flat_map(|verification| &verification.cells)
        .filter(|cell| cell.status == VerificationStatus::Confirmed)
        .map(|cell| format!("{}/{}", cell.group, cell.condition))
        .collect();
    cells.sort();
    cells.dedup();
    cells
}

/// The first reason a finding could not be verified, for the warning text.
pub(crate) fn inconclusive_reason(finding: &ShadowFinding) -> Option<String> {
    finding
        .sources
        .iter()
        .filter_map(|source| source.verification.as_ref())
        .flat_map(|verification| &verification.cells)
        .filter(|cell| cell.status == VerificationStatus::Unverified)
        .find_map(|cell| cell.inconclusive_reason.clone())
}

/// How many dispatch transcripts backed a finding's confirmations.
pub(crate) fn confirming_dispatch_count(finding: &ShadowFinding) -> usize {
    finding
        .sources
        .iter()
        .filter_map(|source| source.verification.as_ref())
        .flat_map(|verification| &verification.cells)
        .filter(|cell| cell.status == VerificationStatus::Confirmed)
        .map(|cell| cell.dispatches_with_evidence)
        .sum()
}

#[cfg(test)]
mod tests;
