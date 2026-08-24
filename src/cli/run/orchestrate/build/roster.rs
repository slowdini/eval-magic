use std::path::Path;

use crate::core::ConditionSkill;
use crate::core::fs::artifact_path;

use super::super::StagedTreatmentSkill;

pub(super) fn condition_roster(
    paths: &[(String, String)],
    staged_skills: &[StagedTreatmentSkill],
) -> Vec<ConditionSkill> {
    paths
        .iter()
        .map(|(name, path)| ConditionSkill {
            name: name.clone(),
            skill_path: artifact_path(Path::new(path)),
            staged_skill_slug: staged_skills
                .iter()
                .find(|skill| &skill.name == name)
                .and_then(|skill| skill.slug.clone()),
        })
        .collect()
}
