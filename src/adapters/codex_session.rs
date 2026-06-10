//! Codex-specific rendering of the skills surface.
//!
//! Codex exposes skills with a name,
//! description, and file path in its initial skills list. Kept separate from
//! Claude Code's Skill-tool wording so dispatch prompts mirror the harness being
//! evaluated.

use crate::core::AvailableSkill;

/// Render the discoverable skills the way Codex surfaces them in its initial
/// skills list: a `## Skills` heading followed by `- name: description (file:
/// path)` bullets. Returns an empty string when no skills are staged.
pub fn render_codex_available_skills_block(skills: &[AvailableSkill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&AvailableSkill> = skills.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::from("## Skills\n");
    for s in sorted {
        out.push_str(&format!(
            "\n- {}: {} (file: {})",
            s.name, s.description, s.path
        ));
    }
    out
}

/// Render a Codex plan-mode profile as an operating-context reminder. Codex's
/// non-interactive `exec` surface has no documented real plan-mode switch, so
/// this mirrors the portable approximation: text injected ahead of the task.
pub fn render_codex_plan_mode_context(profile_text: &str) -> String {
    let trimmed = profile_text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    format!("<system-reminder>\n{trimmed}\n</system-reminder>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::AvailableSkill;

    fn skill(name: &str, description: &str) -> AvailableSkill {
        AvailableSkill {
            name: name.into(),
            path: format!("/x/{name}/SKILL.md"),
            description: description.into(),
        }
    }

    #[test]
    fn renders_codex_surface_with_name_description_and_file_path() {
        let block =
            render_codex_available_skills_block(&[skill("mr-review", "review merge requests")]);
        assert!(block.contains("## Skills"));
        assert!(block.contains("- mr-review: review merge requests"));
        assert!(block.contains("(file: /x/mr-review/SKILL.md)"));
        assert!(!block.contains("The following skills are available for use with the Skill tool:"));
    }

    #[test]
    fn sorts_skills_by_name() {
        let block =
            render_codex_available_skills_block(&[skill("zebra", "z"), skill("alpha", "a")]);
        assert!(block.find("- alpha:").unwrap() < block.find("- zebra:").unwrap());
    }

    #[test]
    fn empty_list_renders_empty_string() {
        assert_eq!(render_codex_available_skills_block(&[]), "");
    }

    #[test]
    fn plan_mode_wraps_in_system_reminder_with_codex_profile_text() {
        let block = render_codex_plan_mode_context("Codex plan mode is active.");
        assert_eq!(
            block,
            "<system-reminder>\nCodex plan mode is active.\n</system-reminder>"
        );
    }
}
