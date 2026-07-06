//! Codex harness support.
//!
//! Everything Codex-specific lives in this module tree: the adapter impl (this
//! file), `codex exec` dispatch-recipe rendering ([`cli`]), the `## Skills`
//! block ([`session`]), `item.completed` event-stream parsing ([`transcript`]),
//! and the write-guard hook ([`guard`]).

mod cli;
pub(crate) mod guard;
pub mod session;
pub mod transcript;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::{AvailableSkill, HarnessRunCapabilities, ToolInvocation};

use super::TranscriptSummary;
use super::harness::{CliDispatchContext, CliJudgeContext, CliManifestContext, HarnessAdapter};
use cli::{
    codex_exec_command_template, codex_judge_dispatch_recipe, codex_parallel_dispatch_recipe,
};
use session::render_codex_available_skills_block;
use transcript::{parse_codex_events, parse_codex_events_full};

pub struct CodexAdapter;

impl HarnessAdapter for CodexAdapter {
    fn label(&self) -> &'static str {
        "codex"
    }
    fn skills_dir(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(".agents").join("skills")
    }
    fn run_capabilities(&self) -> HarnessRunCapabilities {
        HarnessRunCapabilities {
            supports_guard: true,
            supports_bootstrap_with_no_stage: false,
            supports_stage_name_with_no_stage: false,
        }
    }
    fn config_dir_names(&self) -> &'static [&'static str] {
        &[".agents", ".codex"]
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
    // Codex's JSONL exposes no deterministic skill-tool event, so the
    // `__skill_invoked` meta-check uses the LLM-judge fallback.
    fn transcript_surfaces_skill_invocation(&self) -> bool {
        false
    }
    fn install_guard(
        &self,
        stage_root: &Path,
        guard_exe: &Path,
        ttl: Option<Duration>,
    ) -> io::Result<PathBuf> {
        guard::install_guard(stage_root, guard_exe, ttl)
    }
    fn guard_hook_cleanup_dir(&self, stage_root: &Path) -> Option<PathBuf> {
        Some(guard::hook_cleanup_dir(stage_root))
    }
    fn guard_armed_message(&self) -> Option<&'static str> {
        Some(
            "\n🛡 Write guard armed: a PreToolUse hook is staged in .codex/hooks.json\n   and will block writes/installs outside the eval sandbox during Codex dispatches.\n   Dispatch with codex --ask-for-approval never exec --dangerously-bypass-hook-trust so the vetted eval hook runs.\n   It auto-expires in 6h and is removed on the next run; to remove it now:\n     eval-magic teardown-guard",
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::adapters::adapter_for;
    use crate::core::Harness;

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
