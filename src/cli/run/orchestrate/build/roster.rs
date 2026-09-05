use std::path::Path;

use crate::core::fs::artifact_path;
use crate::core::{ConditionSkill, Harness};

use super::super::super::staging::skills_dir_for_harness;
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
            staged_skill_path: None,
        })
        .collect()
}

pub(super) fn staged_skill_path_for(
    env_root: &Path,
    harness: Harness,
    staged_slug: Option<&str>,
) -> Option<String> {
    staged_slug.map(|slug| {
        artifact_path(
            &skills_dir_for_harness(env_root, harness)
                .join(slug)
                .join("SKILL.md"),
        )
    })
}

pub(super) fn task_roster(
    condition_roster: &[ConditionSkill],
    env_root: &Path,
    harness: Harness,
) -> Vec<ConditionSkill> {
    condition_roster
        .iter()
        .map(|skill| ConditionSkill {
            staged_skill_path: staged_skill_path_for(
                env_root,
                harness,
                skill.staged_skill_slug.as_deref(),
            ),
            ..skill.clone()
        })
        .collect()
}
