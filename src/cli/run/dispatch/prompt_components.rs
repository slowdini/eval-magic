use std::fs;
use std::path::Path;

use crate::adapters::adapter_for;
use crate::core::AvailableSkill;

use super::{DispatchTaskOpts, RunError, is_truthy, redact_skill_from_bootstrap};

pub(super) fn render_skill_block(
    opts: &DispatchTaskOpts<'_>,
    skill_path: Option<&str>,
    staged_skill_path: Option<&str>,
    staged_skills: &[AvailableSkill],
) -> Result<String, RunError> {
    if let Some(skills) = opts.skills {
        if skills.is_empty() {
            return Ok(String::new());
        }
        if skills.iter().any(|skill| skill.staged_skill_slug.is_some()) {
            let surface = adapter_for(opts.harness).skill_surface_phrase();
            return Ok(skills
                .iter()
                .map(|skill| {
                    let slug = skill
                        .staged_skill_slug
                        .as_deref()
                        .unwrap_or(skill.name.as_str());
                    format!(
                        "The `{}` skill is registered under the identifier `{slug}` and is discoverable {surface}. If you invoke it, use that identifier.",
                        skill.name
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"));
        }

        let mut sections = vec![
            "The following skills are loaded into your operating guidelines. Apply them where relevant to the user's request."
                .to_string(),
        ];
        for skill in skills {
            let content = fs::read_to_string(&skill.skill_path)?;
            sections.push(format!(
                "\n<skill name=\"{}\">\n{}\n</skill>",
                skill.name,
                content.trim()
            ));
        }
        return Ok(sections.join("\n"));
    }

    if let Some(slug) = opts.staged_skill_slug {
        // The identifier disambiguates the staged copy without implying that it
        // is a globally installed plugin.
        let adapter = adapter_for(opts.harness);
        let surface = adapter.skill_surface_phrase();
        let mut lines = vec![format!(
            "The `{}` skill is registered under the identifier `{slug}` and is discoverable {surface}. If you invoke it, use that identifier.",
            opts.skill_name
        )];
        if let Some(staged_path) = staged_skill_path {
            let cannot_resolve = adapter.skill_unresolved_phrase();
            lines.push(format!(
                "{cannot_resolve}, read the skill from `{staged_path}` instead."
            ));
        }
        return Ok(lines.join("\n"));
    }

    if let Some(skill_path) = skill_path {
        let content = fs::read_to_string(skill_path)?;
        let dir_name = Path::new(skill_path)
            .parent()
            .and_then(Path::file_name)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Ok([
            "The following skill is loaded into your operating guidelines. Apply it where relevant to the user's request.",
            "",
            &format!("<skill name=\"{dir_name}\">"),
            content.trim(),
            "</skill>",
        ]
        .join("\n"));
    }

    if !staged_skills.is_empty() || is_truthy(opts.bootstrap_content) {
        // In a realistic skill-absent arm, the available-skills block already
        // communicates the environment; extra commentary would announce the eval.
        Ok(String::new())
    } else {
        Ok("No skill is loaded. Respond as you naturally would.".to_string())
    }
}

pub(super) fn render_overlay_files_block(files: &[String]) -> String {
    if files.is_empty() {
        "Codebase overlay files: none".to_string()
    } else {
        format!(
            "Codebase overlay files:\n{}",
            files
                .iter()
                .map(|file| format!("  - {file}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

pub(super) fn effective_bootstrap(
    opts: &DispatchTaskOpts<'_>,
    skill_absent: bool,
) -> Option<String> {
    match opts.bootstrap_content {
        Some(content) if !content.is_empty() => Some(if skill_absent {
            opts.treatment_names.map_or_else(
                || redact_skill_from_bootstrap(content, opts.skill_name),
                |names| {
                    names.iter().fold(content.to_string(), |bootstrap, name| {
                        redact_skill_from_bootstrap(&bootstrap, name)
                    })
                },
            )
        } else {
            content.to_string()
        }),
        _ => None,
    }
}
