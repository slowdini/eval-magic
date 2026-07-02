//! The harness adapter API — the single seam between generic dispatch code and
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
use super::claude_cli::{
    claude_exec_command_template, claude_judge_dispatch_recipe, claude_parallel_dispatch_recipe,
};
use super::codex_cli::{
    codex_exec_command_template, codex_judge_dispatch_recipe, codex_parallel_dispatch_recipe,
};
use super::{
    parse_claude_stream_json, parse_claude_stream_json_full, parse_codex_events,
    parse_codex_events_full, render_available_skills_block, render_codex_available_skills_block,
    render_opencode_available_skills_block,
};

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

pub struct ClaudeCodeAdapter;
pub struct CodexAdapter;
pub struct OpenCodeAdapter;

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
    fn cli_events_filename(&self) -> Option<&'static str> {
        Some("claude-events.jsonl")
    }
    fn cli_model_flag(&self) -> Option<&'static str> {
        Some("--model")
    }
    fn cli_next_steps(&self, ctx: CliDispatchContext<'_>) -> String {
        format!(
            "\nNext: iterate the tasks[] array in dispatch.json and dispatch each task (from the env dir — `claude` has no --cd flag) with:\n{}\nThen run `ingest{target_args} --iteration {iteration} --harness claude-code`.",
            claude_exec_command_template(self.cli_model_flag(), ctx.agent_model),
            target_args = ctx.target_args,
            iteration = ctx.iteration
        )
    }
    fn cli_manifest_section(&self, ctx: CliManifestContext<'_>) -> Option<Vec<String>> {
        Some(vec![
            "After all dispatches (Claude Code):".to_string(),
            String::new(),
            "Run one fresh `claude -p` per task from the env dir (`cd <eval-root>` — `claude` has no --cd flag). `--output-format stream-json` requires `--verbose`; detach stdin with `</dev/null` so a permission prompt cannot block and piped task data cannot become extra prompt context; capture stdout as `outputs/claude-events.jsonl` and stderr as `outputs/claude-stderr.log`.".to_string(),
            String::new(),
            "```bash".to_string(),
            claude_exec_command_template(self.cli_model_flag(), ctx.agent_model),
            "```".to_string(),
            String::new(),
            "Parallel dispatch from this iteration directory:".to_string(),
            String::new(),
            "```bash".to_string(),
            claude_parallel_dispatch_recipe(self.cli_model_flag(), ctx.agent_model),
            "```".to_string(),
            String::new(),
            "Then run `eval-magic ingest --harness claude-code`; ingest reads each task's `outputs/claude-events.jsonl`.".to_string(),
            String::new(),
        ])
    }
    fn cli_judge_next_steps(&self, ctx: CliJudgeContext<'_>) -> Option<String> {
        Some(claude_judge_dispatch_recipe(
            self.cli_model_flag(),
            ctx.iteration_dir,
        ))
    }
    fn parse_cli_events(&self, path: &Path) -> io::Result<Vec<ToolInvocation>> {
        parse_claude_stream_json(path)
    }
    fn parse_cli_events_full(&self, path: &Path) -> io::Result<TranscriptSummary> {
        parse_claude_stream_json_full(path)
    }
    fn install_guard(
        &self,
        stage_root: &Path,
        guard_exe: &Path,
        ttl: Option<Duration>,
    ) -> io::Result<PathBuf> {
        crate::sandbox::install::install_claude_guard(stage_root, guard_exe, ttl)
    }
    fn guard_armed_message(&self) -> Option<&'static str> {
        Some(
            "\n🛡 Write guard armed: a PreToolUse hook is staged in .claude/settings.local.json\n   and will block writes/installs outside the eval sandbox during dispatches.\n   Each `claude -p` dispatch loads the hook from the env cwd it runs in.\n   It auto-expires in 6h and is removed on the next run; to remove it now:\n     eval-magic teardown-guard",
        )
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
    fn cli_events_filename(&self) -> Option<&'static str> {
        Some("codex-events.jsonl")
    }
    fn cli_model_flag(&self) -> Option<&'static str> {
        Some("-m")
    }
    fn cli_next_steps(&self, ctx: CliDispatchContext<'_>) -> String {
        format!(
            "\nNext: iterate the tasks[] array in dispatch.json and dispatch each task with:\n{}\nThen run `ingest{target_args} --iteration {iteration} --harness codex`.",
            codex_exec_command_template(self.cli_model_flag(), ctx.guard, ctx.agent_model),
            target_args = ctx.target_args,
            iteration = ctx.iteration
        )
    }
    fn cli_manifest_section(&self, ctx: CliManifestContext<'_>) -> Option<Vec<String>> {
        Some(vec![
            "After all dispatches (Codex):".to_string(),
            String::new(),
            "Run one fresh `codex --ask-for-approval never exec --json` per task. Detach stdin with `</dev/null` so piped task data cannot become extra prompt context; capture stdout as `outputs/codex-events.jsonl` and stderr as `outputs/codex-stderr.log`.".to_string(),
            String::new(),
            "```bash".to_string(),
            codex_exec_command_template(self.cli_model_flag(), ctx.guard, ctx.agent_model),
            "```".to_string(),
            String::new(),
            "Parallel dispatch from this iteration directory:".to_string(),
            String::new(),
            "```bash".to_string(),
            codex_parallel_dispatch_recipe(self.cli_model_flag(), ctx.guard, ctx.agent_model),
            "```".to_string(),
            String::new(),
            "Then run `eval-magic ingest --harness codex`; Codex transcript ingest reads each task's `outputs/codex-events.jsonl`.".to_string(),
            String::new(),
        ])
    }
    fn cli_judge_next_steps(&self, ctx: CliJudgeContext<'_>) -> Option<String> {
        Some(codex_judge_dispatch_recipe(
            self.cli_model_flag(),
            ctx.guard,
            ctx.iteration_dir,
        ))
    }
    fn parse_cli_events(&self, path: &Path) -> io::Result<Vec<ToolInvocation>> {
        parse_codex_events(path)
    }
    fn parse_cli_events_full(&self, path: &Path) -> io::Result<TranscriptSummary> {
        parse_codex_events_full(path)
    }
    fn install_guard(
        &self,
        stage_root: &Path,
        guard_exe: &Path,
        ttl: Option<Duration>,
    ) -> io::Result<PathBuf> {
        crate::sandbox::install::install_codex_guard(stage_root, guard_exe, ttl)
    }
    fn guard_armed_message(&self) -> Option<&'static str> {
        Some(
            "\n🛡 Write guard armed: a PreToolUse hook is staged in .codex/hooks.json\n   and will block writes/installs outside the eval sandbox during Codex dispatches.\n   Dispatch with codex --ask-for-approval never exec --dangerously-bypass-hook-trust so the vetted eval hook runs.\n   It auto-expires in 6h and is removed on the next run; to remove it now:\n     eval-magic teardown-guard",
        )
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

    #[test]
    fn claude_adapter_advertises_cli_events_file_and_model_flag() {
        let a = adapter_for(Harness::ClaudeCode);
        assert_eq!(a.cli_events_filename(), Some("claude-events.jsonl"));
        assert_eq!(a.cli_model_flag(), Some("--model"));
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

    #[test]
    fn claude_parse_cli_events_full_reads_stream_json_result_event() {
        use serde_json::json;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("claude-events.jsonl");
        // No per-line timestamps; the result event is the only source of duration.
        let lines = [
            json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}
            ]}}),
            json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done", "duration_ms": 5637, "usage": {"input_tokens": 1, "output_tokens": 2, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}),
        ];
        let body = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, format!("{body}\n")).unwrap();

        let a = adapter_for(Harness::ClaudeCode);
        let summary = a.parse_cli_events_full(&path).unwrap();
        assert_eq!(summary.final_text, Some("Done".into()));
        assert_eq!(summary.duration_ms, Some(5637));
        assert_eq!(summary.tool_invocations.len(), 1);
        assert_eq!(summary.tool_invocations[0].name, "Bash");
    }

    #[test]
    fn codex_parse_cli_events_delegates_to_events_parser() {
        use serde_json::json;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("codex-events.jsonl");
        let line = json!({"type": "item.completed", "item": {"id": "i1", "type": "command_execution", "command": "bun test", "output": "ok"}});
        std::fs::write(&path, format!("{line}\n")).unwrap();

        let inv = adapter_for(Harness::Codex).parse_cli_events(&path).unwrap();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].name, "command_execution");
    }
}
