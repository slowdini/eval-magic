//! Claude Code-specific rendering of session-start context.
//!
//! The available-skills reminder is
//! a *harness-specific* surface: Claude Code presents discoverable skills to an
//! agent as "The following skills are available for use with the Skill tool:"
//! followed by `- name: description` bullets. Plan-mode context is injected as a
//! `<system-reminder>` block. Both live in an adapter rather than the harness-
//! agnostic orchestrator so a new harness adds its own renderer alongside.

use std::path::{Path, PathBuf};

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

/// Slugify an absolute path the way Claude Code names its project directories:
/// every non-alphanumeric character becomes `-`. For example
/// `/Users/x/.config/oc` → `-Users-x--config-oc` (the `/` before `.config` and
/// the `.` each map to a `-`, producing the double hyphen).
pub fn slugify_project_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Locate the subagents transcript dir for a Claude Code session.
///
/// Returns `<config_dir>/projects/<slug>/<session_id>/subagents/` when it
/// exists, where `<slug>` is [`slugify_project_path`] of `cwd`. If the
/// cwd-derived slug doesn't match (e.g. the command ran from a subdirectory of
/// the session's project), scans `<config_dir>/projects/*` for a child named
/// `<session_id>` — the session id is a globally-unique UUID, so at most one
/// project dir contains it. Returns `None` if no `subagents/` dir is found.
pub fn resolve_subagents_dir_for_session(
    config_dir: &Path,
    cwd: &Path,
    session_id: &str,
) -> Option<PathBuf> {
    let projects = config_dir.join("projects");
    let primary = projects
        .join(slugify_project_path(cwd))
        .join(session_id)
        .join("subagents");
    if primary.is_dir() {
        return Some(primary);
    }
    let entries = std::fs::read_dir(&projects).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(session_id).join("subagents");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::AvailableSkill;
    use std::fs;
    use tempfile::TempDir;

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

    #[test]
    fn slugify_matches_claude_code_double_hyphen() {
        // Verified against a real Claude Code project dir: the `/` before `.config`
        // and the `.` both become `-`, producing a double hyphen.
        assert_eq!(
            slugify_project_path(Path::new("/Users/maxhaarhaus/.config/opencode")),
            "-Users-maxhaarhaus--config-opencode"
        );
    }

    #[test]
    fn slugify_replaces_all_non_alphanumerics_keeping_alnum() {
        assert_eq!(
            slugify_project_path(Path::new("/a-b/c.d_e f")),
            "-a-b-c-d-e-f"
        );
        assert_eq!(slugify_project_path(Path::new("/Proj9/v2")), "-Proj9-v2");
    }

    /// Create `<config>/projects/<dir>/<sid>/subagents/` and return the subagents path.
    fn make_subagents(config: &Path, project_dir: &str, sid: &str) -> std::path::PathBuf {
        let dir = config
            .join("projects")
            .join(project_dir)
            .join(sid)
            .join("subagents");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_finds_primary_cwd_slug_path() {
        let tmp = TempDir::new().unwrap();
        let cwd = Path::new("/tmp/proj");
        let sid = "5ade3f59-dda3-4f40-8776-79f82ba0fab2";
        let expected = make_subagents(tmp.path(), "-tmp-proj", sid);
        assert_eq!(
            resolve_subagents_dir_for_session(tmp.path(), cwd, sid),
            Some(expected)
        );
    }

    #[test]
    fn resolve_falls_back_to_scan_when_cwd_slug_differs() {
        let tmp = TempDir::new().unwrap();
        let cwd = Path::new("/tmp/proj"); // slug `-tmp-proj` is NOT created
        let sid = "11111111-2222-3333-4444-555555555555";
        let expected = make_subagents(tmp.path(), "some-other-project-slug", sid);
        assert_eq!(
            resolve_subagents_dir_for_session(tmp.path(), cwd, sid),
            Some(expected)
        );
    }

    #[test]
    fn resolve_prefers_primary_over_scan_match() {
        let tmp = TempDir::new().unwrap();
        let cwd = Path::new("/tmp/proj");
        let sid = "99999999-aaaa-bbbb-cccc-dddddddddddd";
        // A scan candidate that sorts first, plus the cwd-slug primary.
        make_subagents(tmp.path(), "aaa-other", sid);
        let primary = make_subagents(tmp.path(), "-tmp-proj", sid);
        assert_eq!(
            resolve_subagents_dir_for_session(tmp.path(), cwd, sid),
            Some(primary)
        );
    }

    #[test]
    fn resolve_none_when_session_dir_absent() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("projects")).unwrap();
        assert_eq!(
            resolve_subagents_dir_for_session(tmp.path(), Path::new("/tmp/proj"), "no-such-sid"),
            None
        );
    }

    #[test]
    fn resolve_none_when_subagents_subdir_missing() {
        let tmp = TempDir::new().unwrap();
        let sid = "abcdabcd-0000-1111-2222-333333333333";
        // Session dir exists (under the cwd slug) but without a `subagents/` child.
        fs::create_dir_all(tmp.path().join("projects").join("-tmp-proj").join(sid)).unwrap();
        assert_eq!(
            resolve_subagents_dir_for_session(tmp.path(), Path::new("/tmp/proj"), sid),
            None
        );
    }
}
