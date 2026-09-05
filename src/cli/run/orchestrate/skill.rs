use std::path::Path;

use crate::core::fs::artifact_path;
use crate::core::{SkillSource, SkillSourceEntry, SourceKind, SourceRecord};
use crate::source::ResolvedSource;

/// The resolved treatment and ambient roster staged with it.
pub(super) struct RunSkill {
    pub(super) eval_owner: String,
    /// Whether `evals.json` authored the treatment as a list. This is distinct
    /// from roster length so a one-member list still uses the list artifact form.
    pub(super) multi: bool,
    pub(super) source: ResolvedSource,
    pub(super) treatments: Vec<TreatmentSkill>,
    /// Captured at resolution. Staging copies exactly these names, so what the
    /// record claims and what the environments hold cannot drift apart.
    pub(super) siblings: Vec<String>,
}

pub(super) struct TreatmentSkill {
    pub(super) name: String,
    pub(super) source: ResolvedSource,
}

impl RunSkill {
    pub(super) fn record(&self) -> SkillSource {
        SkillSource {
            source: skill_source_record(&self.source),
            siblings: self.siblings.clone(),
            eval_owner: self.multi.then(|| self.eval_owner.clone()),
            skills: self.multi.then(|| {
                self.treatments
                    .iter()
                    .map(|skill| SkillSourceEntry {
                        name: skill.name.clone(),
                        source: skill_source_record(&skill.source),
                    })
                    .collect()
            }),
        }
    }
}

fn skill_source_record(source: &ResolvedSource) -> SourceRecord {
    SourceRecord {
        kind: SourceKind::Path,
        source: source.source.clone(),
        resolved_path: source
            .resolved_path
            .as_deref()
            .map(|path| artifact_path(Path::new(path))),
        reference: source.reference.clone(),
        revision: source.revision.clone(),
        origin_url: source.origin_url.clone(),
        branch: source.branch.clone(),
        host_local: source.host_local,
        dirty: source.dirty,
    }
}
