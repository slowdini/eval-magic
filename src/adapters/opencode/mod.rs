//! OpenCode harness support.
//!
//! Everything OpenCode-specific lives in this module tree: the adapter impl and
//! slug/naming rules (this file) and the `<available_skills>` XML block
//! ([`session`]). Transcript ingest, the model flag, and the write guard are
//! not wired yet; the adapter's error stubs say so.

pub mod session;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::{AvailableSkill, ToolInvocation};

use super::TranscriptSummary;
use super::harness::{CliDispatchContext, HarnessAdapter};
use session::render_opencode_available_skills_block;

pub struct OpenCodeAdapter;

impl HarnessAdapter for OpenCodeAdapter {
    fn label(&self) -> &'static str {
        "opencode"
    }
    fn skills_dir(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(".opencode").join("skills")
    }
    fn rewrites_frontmatter_name(&self) -> bool {
        true
    }
    fn advertises_staged_slug_name(&self) -> bool {
        false
    }
    fn render_available_skills_block(&self, skills: &[AvailableSkill]) -> String {
        render_opencode_available_skills_block(skills)
    }
    fn skill_surface_phrase(&self) -> &'static str {
        "as an OpenCode skill"
    }
    fn skill_unresolved_phrase(&self) -> &'static str {
        "If it does not load as an OpenCode skill"
    }
    fn cli_next_steps(&self, ctx: CliDispatchContext<'_>) -> String {
        let model_note = if ctx.agent_model.is_some() {
            " Model selection was recorded as provenance, but the OpenCode adapter has no CLI model flag wired yet."
        } else {
            ""
        };
        format!(
            "\nNext: iterate the tasks[] array in dispatch.json and dispatch each task with `opencode run`.{model_note} OpenCode transcript ingest is not yet wired, so assemble each task's `run.json`/`timing.json` manually (or capture `opencode run --format json` / `opencode export` output), then run `ingest{target_args} --iteration {iteration} --harness opencode`.",
            target_args = ctx.target_args,
            iteration = ctx.iteration
        )
    }
    // OpenCode transcript ingest is not yet wired: its `cli_events_filename` is
    // `None`, so the ingest pipeline never reaches these parsers. They error
    // rather than parse until OpenCode ingest lands.
    fn parse_cli_events(&self, _path: &Path) -> io::Result<Vec<ToolInvocation>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "opencode transcript ingest is not yet wired",
        ))
    }
    fn parse_cli_events_full(&self, _path: &Path) -> io::Result<TranscriptSummary> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "opencode transcript ingest is not yet wired",
        ))
    }
    fn install_guard(
        &self,
        _stage_root: &Path,
        _guard_exe: &Path,
        _ttl: Option<Duration>,
    ) -> io::Result<PathBuf> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "--guard is not yet supported for the opencode harness",
        ))
    }
}

/// True when `name` satisfies OpenCode's skill-name rules:
/// - 1–64 characters
/// - lowercase alphanumeric with single-hyphen separators
/// - no leading/trailing/consecutive hyphens
pub(crate) fn is_valid_opencode_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let mut prev = '-';
    for ch in name.chars() {
        if ch == '-' {
            if prev == '-' {
                return false;
            }
        } else if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() {
            return false;
        }
        prev = ch;
    }
    !name.starts_with('-') && !name.ends_with('-')
}

/// Sanitize an arbitrary identifier so it is a valid OpenCode skill name.
fn sanitize_opencode_name(name: &str) -> String {
    let mut out = String::new();
    let mut prev_hyphen = false;
    for ch in name.to_ascii_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            out.push(ch);
            prev_hyphen = false;
        } else if !prev_hyphen {
            out.push('-');
            prev_hyphen = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    while out.starts_with('-') {
        out.remove(0);
    }
    if out.is_empty() {
        out.push_str("skill");
    }
    if out.len() > 64 {
        out.truncate(64);
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

/// Build a slug that is valid for OpenCode's skill directory + frontmatter name
/// constraints. `prefix` is the conspicuous staged-skill prefix, preserved so
/// cleanup prefix-scans still find it.
pub(crate) fn opencode_slug(
    prefix: &str,
    iteration: u32,
    condition: &str,
    skill_name: &str,
) -> String {
    let condition = sanitize_opencode_name(condition);
    let skill = sanitize_opencode_name(skill_name);
    let base = format!("{prefix}{iteration}-{condition}-{skill}");
    if base.len() <= 64 && is_valid_opencode_name(&base) {
        return base;
    }
    // If the combined slug is too long, truncate the skill portion.
    let prefix = format!("{prefix}{iteration}-{condition}-");
    let budget = 64usize.saturating_sub(prefix.len());
    let mut truncated = skill.clone();
    truncated.truncate(budget);
    while truncated.ends_with('-') {
        truncated.pop();
    }
    if truncated.is_empty() {
        truncated.push_str("skill");
    }
    format!("{prefix}{truncated}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "slow-powers-eval-";

    #[test]
    fn opencode_slug_sanitizes_underscores_and_special_characters() {
        assert_eq!(
            opencode_slug(PREFIX, 1, "with_skill", "My_Skill!"),
            "slow-powers-eval-1-with-skill-my-skill"
        );
        assert_eq!(
            opencode_slug(PREFIX, 2, "without_skill", "snake_case"),
            "slow-powers-eval-2-without-skill-snake-case"
        );
    }

    #[test]
    fn opencode_slug_truncates_to_valid_max_length() {
        let very_long = "a".repeat(200);
        let slug = opencode_slug(PREFIX, 1, "with_skill", &very_long);
        assert!(slug.len() <= 64);
        assert!(is_valid_opencode_name(&slug));
        assert!(slug.starts_with("slow-powers-eval-1-with-skill-"));
    }
}
