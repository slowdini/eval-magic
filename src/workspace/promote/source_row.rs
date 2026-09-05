use crate::core::SkillSource;

pub(super) fn multi_skill_source_row(skill_source: &SkillSource) -> Option<String> {
    let skills = skill_source.skills.as_ref()?;
    let members = skills
        .iter()
        .map(|skill| {
            let source = &skill.source;
            let mut item = format!(
                "{}: {}",
                skill.name,
                source
                    .resolved_path
                    .clone()
                    .unwrap_or_else(|| source.source.clone())
            );
            if let Some(revision) = &source.revision {
                item.push_str(&format!(
                    " ({})",
                    revision.chars().take(7).collect::<String>()
                ));
            }
            if source.dirty {
                item.push_str(" — uncommitted changes were in what ran");
            }
            if let Some(origin) = &source.origin_url {
                item.push_str(&format!("; origin {origin}"));
            }
            item
        })
        .collect::<Vec<_>>()
        .join("<br>");
    let ambient = if skill_source.siblings.is_empty() {
        String::new()
    } else {
        format!("; ambient skills: {}", skill_source.siblings.join(", "))
    };
    Some(format!("| Skill sources | {members}{ambient} |"))
}
