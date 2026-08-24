//! Group concrete discovery sources into logical skill findings.

use super::*;

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
                    class: ShadowFindingClass::OperatorEnvironment,
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
        Self::from_observed_sources_for_subjects_with_class(
            config_dir,
            sources,
            &[subject_skill_name],
            expected_cells,
            ShadowFindingClass::OperatorEnvironment,
        )
    }

    pub(crate) fn from_observed_sources_with_class(
        config_dir: impl Into<String>,
        sources: Vec<ShadowSource>,
        subject_skill_name: &str,
        expected_cells: &[(String, String)],
        class: ShadowFindingClass,
    ) -> Self {
        Self::from_observed_sources_for_subjects_with_class(
            config_dir,
            sources,
            &[subject_skill_name],
            expected_cells,
            class,
        )
    }

    pub(crate) fn from_observed_sources_for_subjects_with_class(
        config_dir: impl Into<String>,
        sources: Vec<ShadowSource>,
        subject_skill_names: &[&str],
        expected_cells: &[(String, String)],
        class: ShadowFindingClass,
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
            finding.class = class;
            finding.role = if subject_skill_names.contains(&finding.skill_name.as_str()) {
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
