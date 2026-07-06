//! Claude Code-specific rendering of session-start context.
//!
//! The available-skills reminder is
//! a *harness-specific* surface: Claude Code presents discoverable skills to an
//! agent as "The following skills are available for use with the Skill tool:"
//! followed by `- name: description` bullets. It lives in an adapter rather than
//! the harness-agnostic orchestrator so a new harness adds its own renderer
//! alongside.

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
}
