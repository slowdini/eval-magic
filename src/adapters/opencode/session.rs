//! OpenCode-specific rendering of session-start context.
//!
//! OpenCode exposes discoverable skills through the `skill` tool description as
//! `<available_skills>` XML, and loads them from `.opencode/skills/`. This
//! adapter mirrors that native presentation so eval dispatches feel like a real
//! OpenCode session rather than an eval-specific bulletin.

use crate::core::AvailableSkill;

/// Render the discoverable skills the way OpenCode surfaces them in the `skill`
/// tool description: an `<available_skills>` block with one `<skill>` element
/// per skill containing `<name>` and `<description>`. Returns an empty string
/// when no skills are staged.
pub fn render_opencode_available_skills_block(skills: &[AvailableSkill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&AvailableSkill> = skills.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::from("<available_skills>");
    for s in sorted {
        out.push_str(&format!(
            "\n  <skill>\n    <name>{}</name>\n    <description>{}</description>\n  </skill>",
            s.name, s.description
        ));
    }
    out.push_str("\n</available_skills>");
    out
}

/// Render an OpenCode plan-mode profile as an operating-context reminder. The
/// real OpenCode plan agent is a primary agent mode, not text, so this is the
/// portable approximation: a `<system-reminder>` the dispatch reads.
pub fn render_opencode_plan_mode_context(profile_text: &str) -> String {
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
    fn renders_opencode_xml_with_name_and_description() {
        let block =
            render_opencode_available_skills_block(&[skill("git-release", "Create releases")]);
        assert!(block.contains("<available_skills>"));
        assert!(block.contains("</available_skills>"));
        assert!(block.contains("<name>git-release</name>"));
        assert!(block.contains("<description>Create releases</description>"));
        assert!(block.contains("<skill>"));
        assert!(block.contains("</skill>"));
    }

    #[test]
    fn sorts_skills_by_name() {
        let block =
            render_opencode_available_skills_block(&[skill("zebra", "z"), skill("alpha", "a")]);
        assert!(
            block.find("<name>alpha</name>").unwrap() < block.find("<name>zebra</name>").unwrap()
        );
    }

    #[test]
    fn empty_list_renders_empty_string() {
        assert_eq!(render_opencode_available_skills_block(&[]), "");
    }

    #[test]
    fn plan_mode_wraps_in_system_reminder() {
        let block = render_opencode_plan_mode_context("OpenCode plan mode is active.");
        assert_eq!(
            block,
            "<system-reminder>\nOpenCode plan mode is active.\n</system-reminder>"
        );
    }

    #[test]
    fn plan_mode_trims_surrounding_whitespace() {
        let block = render_opencode_plan_mode_context("\n\n  PROFILE-BODY  \n\n");
        assert_eq!(block, "<system-reminder>\nPROFILE-BODY\n</system-reminder>");
    }

    #[test]
    fn plan_mode_empty_or_whitespace_renders_empty_string() {
        assert_eq!(render_opencode_plan_mode_context(""), "");
        assert_eq!(render_opencode_plan_mode_context("   \n  "), "");
    }
}
