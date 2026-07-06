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
}
