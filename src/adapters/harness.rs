//! The harness adapter API — the single seam between generic dispatch code and
//! harness-specific behavior.
//!
//! Every harness-specific concern hangs off the [`HarnessAdapter`] trait: how
//! discoverable skills are presented in a dispatch prompt, how a persisted
//! transcript is parsed, where staged skills live, and which native hook the
//! write guard installs. Generic code resolves an adapter with [`adapter_for`]
//! and then calls the trait — so [`adapter_for`] is the one place that names a
//! concrete harness for this surface. The impls live in the per-harness
//! modules ([`claude_code`](super::claude_code), [`codex`](super::codex),
//! [`opencode`](super::opencode)).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::{AvailableSkill, Harness, ToolInvocation};

use super::TranscriptSummary;

/// The behavior that varies by harness. Generic dispatch code depends on this
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

    /// Wrap a plan-mode profile as a `<system-reminder>` operating-context
    /// layer. The default usually suffices.
    fn render_plan_mode_context(&self, profile_text: &str) -> String {
        let trimmed = profile_text.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        format!("<system-reminder>\n{trimmed}\n</system-reminder>")
    }

    /// The filename (under a task's `outputs/` dir) this harness's one-shot CLI
    /// writes the captured transcript to. `None` when the harness has no
    /// transcript ingest wired yet (e.g. OpenCode).
    fn cli_events_filename(&self) -> Option<&'static str> {
        None
    }

    /// The native model-selection flag accepted by this harness's CLI. `None`
    /// means the adapter has no model-selection support wired yet.
    fn cli_model_flag(&self) -> Option<&'static str> {
        None
    }

    /// The `Next:` guidance printed after `run`: how to dispatch each task through
    /// this harness's one-shot CLI and then ingest. Empty when the adapter has no
    /// dispatch recipe wired.
    fn cli_next_steps(&self, _ctx: CliDispatchContext<'_>) -> String {
        String::new()
    }

    /// Extra `dispatch-manifest.md` lines describing this harness's dispatch
    /// recipe (command template, parallel recipe, ingest note). `None` when the
    /// harness contributes no manifest section.
    fn cli_manifest_section(&self, _ctx: CliManifestContext<'_>) -> Option<Vec<String>> {
        None
    }

    /// The post-`grade` / post-`ingest` judge dispatch guidance for this harness.
    /// `None` leaves the generic judge handoff in place.
    fn cli_judge_next_steps(&self, _ctx: CliJudgeContext<'_>) -> Option<String> {
        None
    }

    /// Parse the events file this harness's one-shot CLI wrote (the captured
    /// transcript) into ordered tool invocations.
    fn parse_cli_events(&self, path: &Path) -> io::Result<Vec<ToolInvocation>>;

    /// The full-summary counterpart of [`parse_cli_events`](Self::parse_cli_events):
    /// tool invocations, deduped token usage, duration, and final message text.
    fn parse_cli_events_full(&self, path: &Path) -> io::Result<TranscriptSummary>;

    /// Arm the write guard using this harness's native pre-tool hook surface,
    /// returning the staged marker path. The guard's allowed roots are derived
    /// from `stage_root` (the isolated env / agent cwd), so it bounds the agent to
    /// the same env boundary that isolates its reads.
    fn install_guard(
        &self,
        stage_root: &Path,
        guard_exe: &Path,
        ttl: Option<Duration>,
    ) -> io::Result<PathBuf>;

    /// The banner printed after `--guard` successfully arms, describing the
    /// harness's native hook surface and how to remove it. Harness-specific text,
    /// so it lives here rather than in generic run code. `None` for a harness with
    /// no write guard (its [`install_guard`](Self::install_guard) errors), in which
    /// case no banner is printed.
    fn guard_armed_message(&self) -> Option<&'static str> {
        None
    }
}

/// The shared (human-followed) `RUNBOOK.md` template used by every run,
/// regardless of harness (Claude Code, Codex, OpenCode).
pub const RUNBOOK_TEMPLATE: &str = include_str!("../../profiles/shared/runbook.md");

/// Context for rendering a harness's one-shot CLI agent-dispatch guidance.
#[derive(Debug, Clone, Copy)]
pub struct CliDispatchContext<'a> {
    pub guard: bool,
    pub target_args: &'a str,
    pub iteration: u32,
    pub agent_model: Option<&'a str>,
}

/// Context for rendering a harness's `dispatch-manifest.md` CLI recipe.
#[derive(Debug, Clone, Copy)]
pub struct CliManifestContext<'a> {
    pub guard: bool,
    pub agent_model: Option<&'a str>,
}

/// Context for rendering a harness's one-shot CLI judge-dispatch guidance.
#[derive(Debug, Clone, Copy)]
pub struct CliJudgeContext<'a> {
    pub guard: bool,
    pub iteration_dir: &'a Path,
}

/// Resolve the adapter for a [`Harness`]. This is the single dispatch point on
/// the harness variant for all harness-specific behavior; every other module
/// goes through the returned trait object.
pub fn adapter_for(harness: Harness) -> &'static dyn HarnessAdapter {
    match harness {
        Harness::ClaudeCode => &super::claude_code::ClaudeCodeAdapter,
        Harness::Codex => &super::codex::CodexAdapter,
        Harness::OpenCode => &super::opencode::OpenCodeAdapter,
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
        for h in Harness::ALL {
            let out = adapter_for(h).render_plan_mode_context("BODY");
            assert_eq!(out, "<system-reminder>\nBODY\n</system-reminder>");
            assert_eq!(adapter_for(h).render_plan_mode_context("   "), "");
        }
    }

    #[test]
    fn guard_armed_message_is_harness_specific_and_absent_for_opencode() {
        // The post-arm `--guard` banner names the harness's native hook surface,
        // so it lives behind the adapter rather than in generic run code.
        let claude = adapter_for(Harness::ClaudeCode)
            .guard_armed_message()
            .expect("claude code has a write guard");
        assert!(
            claude.contains(".claude/settings.local.json"),
            "claude banner names its hook file: {claude}"
        );

        let codex = adapter_for(Harness::Codex)
            .guard_armed_message()
            .expect("codex has a write guard");
        assert!(
            codex.contains(".codex/hooks.json"),
            "codex banner names its hook file: {codex}"
        );

        // OpenCode has no write guard (its install_guard errors), so there is no
        // banner to print.
        assert_eq!(adapter_for(Harness::OpenCode).guard_armed_message(), None);
    }
}
