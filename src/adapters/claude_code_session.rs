//! Claude Code-specific rendering of session-start context.
//!
//! Ports `src/adapters/claude-code-session.ts`. The available-skills reminder is
//! a *harness-specific* surface: Claude Code presents discoverable skills to an
//! agent as "The following skills are available for use with the Skill tool:"
//! followed by `- name: description` bullets. Plan-mode context is injected as a
//! `<system-reminder>` block. Both live in an adapter rather than the harness-
//! agnostic orchestrator so a new harness adds its own renderer alongside.

use crate::core::AvailableSkill;

/// Render the list of discoverable skills the way a real Claude Code session
/// surfaces them, so an eval dispatch mirrors a genuine session rather than
/// announcing itself as an eval. Returns an empty string when no skills are
/// staged (the caller omits the block entirely in that case).
pub fn render_available_skills_block(skills: &[AvailableSkill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&AvailableSkill> = skills.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::from("The following skills are available for use with the Skill tool:\n");
    for s in sorted {
        out.push_str(&format!("\n- {}: {}", s.name, s.description));
    }
    out
}

/// Render a plan-mode profile the way Claude Code injects an operating mode into
/// a live session: as a `<system-reminder>` block the agent is told it is
/// operating under, not prose it merely reads. Returns an empty string for empty
/// input so the caller can omit the section entirely.
pub fn render_plan_mode_context(profile_text: &str) -> String {
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
    fn uses_harness_native_header_and_one_bullet_per_skill() {
        let block = render_available_skills_block(&[skill("foo", "the foo skill")]);
        assert!(block.contains("The following skills are available for use with the Skill tool:"));
        assert!(block.contains("- foo: the foo skill"));
        // The eval-flavored wording and custom format must be gone.
        assert!(!block.contains("staged and discoverable"));
        assert!(!block.contains("*Trigger:*"));
    }

    #[test]
    fn sorts_skills_by_name() {
        let block = render_available_skills_block(&[skill("zebra", "z"), skill("alpha", "a")]);
        assert!(block.find("- alpha:").unwrap() < block.find("- zebra:").unwrap());
    }

    #[test]
    fn empty_list_renders_empty_string() {
        assert_eq!(render_available_skills_block(&[]), "");
    }

    #[test]
    fn plan_mode_wraps_in_system_reminder() {
        let block = render_plan_mode_context("Plan mode is active. Do not edit.");
        assert!(block.contains("<system-reminder>"));
        assert!(block.contains("</system-reminder>"));
        assert!(block.contains("Plan mode is active. Do not edit."));
    }

    #[test]
    fn plan_mode_trims_surrounding_whitespace() {
        let block = render_plan_mode_context("\n\n  PROFILE-BODY  \n\n");
        assert_eq!(block, "<system-reminder>\nPROFILE-BODY\n</system-reminder>");
    }

    #[test]
    fn plan_mode_empty_or_whitespace_renders_empty_string() {
        assert_eq!(render_plan_mode_context(""), "");
        assert_eq!(render_plan_mode_context("   \n  "), "");
    }
}
