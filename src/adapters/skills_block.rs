//! The generic available-skills block renderer.
//!
//! Every harness surfaces discoverable skills as header + one item per skill
//! (+ optional footer); only the strings differ, so they live in each harness
//! descriptor's `[skills_block]` and this renderer supplies the shared shape:
//! empty list renders empty, skills sort by name, and the item template's
//! `{name}`/`{description}`/`{path}` placeholders fill per skill.

use crate::core::AvailableSkill;

use super::descriptor::subst;

/// The neutral shape used when a descriptor declares no `[skills_block]` —
/// the trait's documented default.
pub(crate) const DEFAULT_HEADER: &str = "The following skills are available in this session:\n";
pub(crate) const DEFAULT_ITEM: &str = "\n- {name}: {description}";

/// Render the block, or an empty string when no skills are staged (the caller
/// omits the block entirely in that case).
pub(crate) fn render_skills_block(
    header: &str,
    item: &str,
    footer: &str,
    skills: &[AvailableSkill],
) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&AvailableSkill> = skills.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::from(header);
    for skill in sorted {
        out.push_str(&subst(
            item,
            &[
                ("name", &skill.name),
                ("description", &skill.description),
                ("path", &skill.path),
            ],
        ));
    }
    out.push_str(footer);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, description: &str) -> AvailableSkill {
        AvailableSkill {
            name: name.into(),
            path: format!("/x/{name}/SKILL.md"),
            description: description.into(),
        }
    }

    #[test]
    fn renders_header_items_and_footer_sorted_by_name() {
        let block = render_skills_block(
            "<skills>",
            "\n- {name}: {description} ({path})",
            "\n</skills>",
            &[skill("zebra", "z"), skill("alpha", "a")],
        );
        assert_eq!(
            block,
            "<skills>\n- alpha: a (/x/alpha/SKILL.md)\n- zebra: z (/x/zebra/SKILL.md)\n</skills>"
        );
    }

    #[test]
    fn empty_list_renders_empty_string_even_with_templates() {
        assert_eq!(render_skills_block("H", "\n- {name}", "F", &[]), "");
    }

    #[test]
    fn braces_in_a_description_are_not_reexpanded() {
        let block = render_skills_block(
            "",
            "\n- {name}: {description}",
            "",
            &[skill("a", "uses {name} literally")],
        );
        assert_eq!(block, "\n- a: uses {name} literally");
    }
}
