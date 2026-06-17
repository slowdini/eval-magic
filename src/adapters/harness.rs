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

    /// For a [`Cli`](crate::core::DispatchMechanism::Cli)-dispatch harness, the
    /// filename (under a task's `outputs/` dir) its one-shot CLI writes the
    /// transcript to. `None` when the harness dispatches in-session (no local
    /// transcript) or has no Cli-mechanism transcript wired yet.
    fn cli_events_filename(&self) -> Option<&'static str> {
        None
    }

    /// The `Next:` guidance printed after `run` for a
    /// [`Cli`](crate::core::DispatchMechanism::Cli)-dispatch harness: how to
    /// dispatch each task through this harness's one-shot CLI and then ingest.
    /// Empty for in-session harnesses (their guidance is the mechanism's, not the
    /// adapter's).
    fn cli_next_steps(&self, _guard: bool, _target_args: &str, _iteration: u32) -> String {
        String::new()
    }

    /// Extra `dispatch-manifest.md` lines describing this harness's Cli dispatch
    /// recipe (command template, parallel recipe, ingest note). `None` when the
    /// harness contributes no Cli-specific manifest section.
    fn cli_manifest_section(&self, _guard: bool) -> Option<Vec<String>> {
        None
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
    fn cli_events_filename(&self) -> Option<&'static str> {
        Some("codex-events.jsonl")
    }
    fn cli_next_steps(&self, guard: bool, target_args: &str, iteration: u32) -> String {
        format!(
            "\nNext: iterate the tasks[] array in dispatch.json and dispatch each task with:\n{}\nThen run `ingest{target_args} --iteration {iteration} --harness codex`.",
            codex_exec_command_template(guard)
        )
    }
    fn cli_manifest_section(&self, guard: bool) -> Option<Vec<String>> {
        Some(vec![
            "After all dispatches (Codex):".to_string(),
            String::new(),
            "Run one fresh `codex exec --json` per task. Detach stdin with `</dev/null` so piped task data cannot become extra prompt context; capture stdout as `outputs/codex-events.jsonl` and stderr as `outputs/codex-stderr.log`.".to_string(),
            String::new(),
            "```bash".to_string(),
            codex_exec_command_template(guard),
            "```".to_string(),
            String::new(),
            "Parallel dispatch from this iteration directory:".to_string(),
            String::new(),
            "```bash".to_string(),
            codex_parallel_dispatch_recipe(guard),
            "```".to_string(),
            String::new(),
            "Then run `eval-magic ingest --harness codex`; Codex transcript ingest reads each task's `outputs/codex-events.jsonl`.".to_string(),
            String::new(),
        ])
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
    fn cli_next_steps(&self, _guard: bool, target_args: &str, iteration: u32) -> String {
        format!(
            "\nNext: iterate the tasks[] array in dispatch.json and dispatch each task with `opencode run`. OpenCode transcript ingest is not yet wired, so assemble each task's `run.json`/`timing.json` manually (or capture `opencode run --format json` / `opencode export` output), then run `ingest{target_args} --iteration {iteration} --harness opencode`."
        )
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

/// Copy/pasteable Codex dispatch command template. Stdin is detached so a
/// surrounding `xargs`/pipe cannot be treated as extra prompt context.
fn codex_exec_command_template(guard: bool) -> String {
    let hook_trust = if guard {
        " --dangerously-bypass-hook-trust"
    } else {
        ""
    };
    [
        format!(
            "codex exec --cd <eval-root> --sandbox workspace-write --ask-for-approval never{hook_trust} --json \\"
        ),
        "  --output-last-message <outputs_dir>/final-message.md \\".to_string(),
        "  \"Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to <outputs_dir>/final-message.md.\" \\".to_string(),
        "  </dev/null \\".to_string(),
        "  > <outputs_dir>/codex-events.jsonl \\".to_string(),
        "  2> <outputs_dir>/codex-stderr.log".to_string(),
    ]
    .join("\n")
}

fn codex_parallel_dispatch_recipe(guard: bool) -> String {
    let hook_trust = if guard {
        " --dangerously-bypass-hook-trust"
    } else {
        ""
    };
    [
        "JOBS=${JOBS:-4}".to_string(),
        "jq -j '.tasks[] | [.dispatch_prompt_path, .outputs_dir] | @tsv + \"\\u0000\"' dispatch.json | \\".to_string(),
        "  xargs -0 -P \"$JOBS\" -I{} sh -c '".to_string(),
        "    prompt_path=\"$(printf \"%s\" \"$1\" | cut -f1)\"".to_string(),
        "    outputs_dir=\"$(printf \"%s\" \"$1\" | cut -f2)\"".to_string(),
        "    mkdir -p \"$outputs_dir\"".to_string(),
        format!(
            "    codex exec --cd <eval-root> --sandbox workspace-write --ask-for-approval never{hook_trust} --json \\"
        ),
        "      --output-last-message \"$outputs_dir/final-message.md\" \\".to_string(),
        "      \"Read the file at $prompt_path and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to $outputs_dir/final-message.md.\" \\".to_string(),
        "      </dev/null \\".to_string(),
        "      > \"$outputs_dir/codex-events.jsonl\" \\".to_string(),
        "      2> \"$outputs_dir/codex-stderr.log\"".to_string(),
        "  ' sh {}".to_string(),
    ]
    .join("\n")
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
