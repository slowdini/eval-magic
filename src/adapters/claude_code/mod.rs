//! Claude Code harness support — the default harness.
//!
//! Everything Claude-Code-specific lives in this module tree: the adapter impl
//! (this file), dispatch-recipe rendering ([`cli`]), the native skills block
//! ([`session`]), `claude -p` stream-json transcript parsing ([`stream_json`] +
//! [`transcript`]), plugin-shadow detection ([`plugin_shadow`]), and the
//! write-guard hook ([`guard`]).

mod cli;
pub(crate) mod guard;
pub mod plugin_shadow;
pub mod session;
pub mod stream_json;
pub mod transcript;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::{AvailableSkill, ToolInvocation};

use super::TranscriptSummary;
use super::harness::{CliDispatchContext, CliJudgeContext, CliManifestContext, HarnessAdapter};
use cli::{
    claude_exec_command_template, claude_judge_dispatch_recipe, claude_parallel_dispatch_recipe,
};
use session::render_available_skills_block;
use stream_json::{parse_claude_stream_json, parse_claude_stream_json_full};

pub struct ClaudeCodeAdapter;

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
        guard::install_guard(stage_root, guard_exe, ttl)
    }
    fn guard_armed_message(&self) -> Option<&'static str> {
        Some(
            "\n🛡 Write guard armed: a PreToolUse hook is staged in .claude/settings.local.json\n   and will block writes/installs outside the eval sandbox during dispatches.\n   Each `claude -p` dispatch loads the hook from the env cwd it runs in.\n   It auto-expires in 6h and is removed on the next run; to remove it now:\n     eval-magic teardown-guard",
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::adapters::adapter_for;
    use crate::core::Harness;

    #[test]
    fn claude_adapter_advertises_cli_events_file_and_model_flag() {
        let a = adapter_for(Harness::ClaudeCode);
        assert_eq!(a.cli_events_filename(), Some("claude-events.jsonl"));
        assert_eq!(a.cli_model_flag(), Some("--model"));
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
}
