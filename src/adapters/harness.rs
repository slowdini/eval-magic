//! The harness adapter API — the single seam between generic run-mode code and
//! harness-specific behavior.
//!
//! Every harness-specific concern hangs off the [`HarnessAdapter`] trait: how
//! discoverable skills are presented in a dispatch prompt, how a persisted
//! transcript is parsed, where staged skills live, and which native hook the
//! write guard installs. Generic code resolves an adapter with [`adapter_for`]
//! and then calls the trait — so [`adapter_for`] is the one place that names a
//! concrete harness for this surface.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::{AvailableSkill, Harness, ToolInvocation};

use super::TranscriptSummary;
use super::{
    parse_codex_events, parse_codex_events_full, parse_transcript, parse_transcript_full,
    render_available_skills_block, render_codex_available_skills_block,
    render_opencode_available_skills_block,
};

/// The behavior that varies by harness. Generic run-mode code depends on this
/// trait, never on a concrete harness variant.
pub trait HarnessAdapter {
    /// The kebab-case identifier used in CLI flags, `dispatch.json`, and the
    /// staged `conditions.json`.
    fn label(&self) -> &'static str;

    /// The project-local directory staged skills live under for this harness.
    fn skills_dir(&self, repo_root: &Path) -> PathBuf;

    /// Whether a staged skill's frontmatter `name:` is rewritten to its slug so
    /// the harness's repo-local discovery resolves the staged copy.
    fn rewrites_frontmatter_name(&self) -> bool;

    /// Whether the skill-under-test is advertised in the available-skills block
    /// under its staged slug (vs. its natural name). True for Codex, whose
    /// repo-local discovery keys on the rewritten frontmatter name. (OpenCode
    /// also rewrites the frontmatter to the slug yet still advertises the natural
    /// name — a known inconsistency tracked for a separate fix.)
    fn advertises_staged_slug_name(&self) -> bool;

    /// Render the discoverable skills the way this harness natively surfaces
    /// them (e.g. Claude Code's Skill-tool list, Codex's `## Skills`, OpenCode's
    /// `<available_skills>` XML).
    fn render_available_skills_block(&self, skills: &[AvailableSkill]) -> String;

    /// How a staged skill is described as discoverable in the neutral
    /// slug-disambiguation line (e.g. "via the Skill tool").
    fn skill_surface_phrase(&self) -> &'static str;

    /// The lead-in for the fallback "read the skill from `<path>`" instruction
    /// when the staged identifier can't be resolved.
    fn skill_unresolved_phrase(&self) -> &'static str;

    /// The verbatim plan-mode procedure profile bundled for this harness.
    fn plan_mode_profile(&self) -> &'static str;

    /// Wrap a plan-mode profile as a `<system-reminder>` operating-context
    /// layer. The default usually suffices.
    fn render_plan_mode_context(&self, profile_text: &str) -> String {
        let trimmed = profile_text.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        format!("<system-reminder>\n{trimmed}\n</system-reminder>")
    }

    /// Parse a persisted transcript into its ordered tool invocations.
    fn parse_transcript(&self, path: &Path) -> io::Result<Vec<ToolInvocation>>;

    /// Parse a persisted transcript into the full summary: tool invocations,
    /// deduped token usage, duration, and final message text.
    fn parse_transcript_full(&self, path: &Path) -> io::Result<TranscriptSummary>;

    /// Arm the write guard using this harness's native pre-tool hook surface,
    /// returning the staged marker path.
    fn install_guard(
        &self,
        stage_root: &Path,
        workspace_root: &Path,
        guard_exe: &Path,
        ttl: Option<Duration>,
    ) -> io::Result<PathBuf>;
}

pub struct ClaudeCodeAdapter;
pub struct CodexAdapter;
pub struct OpenCodeAdapter;

impl HarnessAdapter for ClaudeCodeAdapter {
    fn label(&self) -> &'static str {
        "claude-code"
    }
    fn skills_dir(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(".claude").join("skills")
    }
    fn rewrites_frontmatter_name(&self) -> bool {
        false
    }
    fn advertises_staged_slug_name(&self) -> bool {
        false
    }
    fn render_available_skills_block(&self, skills: &[AvailableSkill]) -> String {
        render_available_skills_block(skills)
    }
    fn skill_surface_phrase(&self) -> &'static str {
        "via the Skill tool"
    }
    fn skill_unresolved_phrase(&self) -> &'static str {
        "If the Skill tool cannot resolve that identifier"
    }
    fn plan_mode_profile(&self) -> &'static str {
        include_str!("../../profiles/claude-code/plan-mode.md")
    }
    fn parse_transcript(&self, path: &Path) -> io::Result<Vec<ToolInvocation>> {
        parse_transcript(path)
    }
    fn parse_transcript_full(&self, path: &Path) -> io::Result<TranscriptSummary> {
        parse_transcript_full(path)
    }
    fn install_guard(
        &self,
        stage_root: &Path,
        workspace_root: &Path,
        guard_exe: &Path,
        ttl: Option<Duration>,
    ) -> io::Result<PathBuf> {
        crate::sandbox::install::install_claude_guard(stage_root, workspace_root, guard_exe, ttl)
    }
}

impl HarnessAdapter for CodexAdapter {
    fn label(&self) -> &'static str {
        "codex"
    }
    fn skills_dir(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(".agents").join("skills")
    }
    fn rewrites_frontmatter_name(&self) -> bool {
        true
    }
    fn advertises_staged_slug_name(&self) -> bool {
        true
    }
    fn render_available_skills_block(&self, skills: &[AvailableSkill]) -> String {
        render_codex_available_skills_block(skills)
    }
    fn skill_surface_phrase(&self) -> &'static str {
        "as a Codex skill"
    }
    fn skill_unresolved_phrase(&self) -> &'static str {
        "If it does not load as a Codex skill"
    }
    fn plan_mode_profile(&self) -> &'static str {
        include_str!("../../profiles/codex/plan-mode.md")
    }
    fn parse_transcript(&self, path: &Path) -> io::Result<Vec<ToolInvocation>> {
        parse_codex_events(path)
    }
    fn parse_transcript_full(&self, path: &Path) -> io::Result<TranscriptSummary> {
        parse_codex_events_full(path)
    }
    fn install_guard(
        &self,
        stage_root: &Path,
        workspace_root: &Path,
        guard_exe: &Path,
        ttl: Option<Duration>,
    ) -> io::Result<PathBuf> {
        crate::sandbox::install::install_codex_guard(stage_root, workspace_root, guard_exe, ttl)
    }
}

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
    fn plan_mode_profile(&self) -> &'static str {
        include_str!("../../profiles/opencode/plan-mode.md")
    }
    // OpenCode transcript ingest is not yet wired. In the current dispatch flow
    // this is unreachable (no subagents dir and no events file), so delegating to
    // the shared JSONL parser preserves the pre-refactor behavior of the
    // transcript-source branch until OpenCode ingest lands.
    fn parse_transcript(&self, path: &Path) -> io::Result<Vec<ToolInvocation>> {
        parse_transcript(path)
    }
    fn parse_transcript_full(&self, path: &Path) -> io::Result<TranscriptSummary> {
        parse_transcript_full(path)
    }
    fn install_guard(
        &self,
        _stage_root: &Path,
        _workspace_root: &Path,
        _guard_exe: &Path,
        _ttl: Option<Duration>,
    ) -> io::Result<PathBuf> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "--guard is not yet supported for the opencode harness",
        ))
    }
}

/// Resolve the adapter for a [`Harness`]. This is the single dispatch point on
/// the harness variant for all harness-specific behavior; every other module
/// goes through the returned trait object.
pub fn adapter_for(harness: Harness) -> &'static dyn HarnessAdapter {
    match harness {
        Harness::ClaudeCode => &ClaudeCodeAdapter,
        Harness::Codex => &CodexAdapter,
        Harness::OpenCode => &OpenCodeAdapter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_kebab_case_identifiers() {
        assert_eq!(adapter_for(Harness::ClaudeCode).label(), "claude-code");
        assert_eq!(adapter_for(Harness::Codex).label(), "codex");
        assert_eq!(adapter_for(Harness::OpenCode).label(), "opencode");
    }

    #[test]
    fn skills_dir_is_harness_native() {
        let root = Path::new("/repo");
        assert_eq!(
            adapter_for(Harness::ClaudeCode).skills_dir(root),
            root.join(".claude").join("skills")
        );
        assert_eq!(
            adapter_for(Harness::Codex).skills_dir(root),
            root.join(".agents").join("skills")
        );
        assert_eq!(
            adapter_for(Harness::OpenCode).skills_dir(root),
            root.join(".opencode").join("skills")
        );
    }

    #[test]
    fn only_codex_and_opencode_rewrite_frontmatter() {
        assert!(!adapter_for(Harness::ClaudeCode).rewrites_frontmatter_name());
        assert!(adapter_for(Harness::Codex).rewrites_frontmatter_name());
        assert!(adapter_for(Harness::OpenCode).rewrites_frontmatter_name());
    }

    #[test]
    fn plan_mode_context_wraps_in_system_reminder_for_every_harness() {
        for h in [Harness::ClaudeCode, Harness::Codex, Harness::OpenCode] {
            let out = adapter_for(h).render_plan_mode_context("BODY");
            assert_eq!(out, "<system-reminder>\nBODY\n</system-reminder>");
            assert_eq!(adapter_for(h).render_plan_mode_context("   "), "");
        }
    }
}
