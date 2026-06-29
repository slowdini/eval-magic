//! The harness adapter layer.
//!
//! [`harness`] defines the [`HarnessAdapter`] trait — the single API generic
//! run-mode code uses to reach harness-specific behavior. The per-harness
//! session renderers + transcript parsers it delegates to live in the sibling
//! submodules, plus plugin-shadow detection. The submodules are re-exported
//! flat so downstream code writes `crate::adapters::<fn>`.

mod claude_cli;
pub mod claude_code_session;
pub mod claude_code_transcript;
pub mod claude_stream_json;
mod cli_command;
mod codex_cli;
pub mod codex_session;
pub mod codex_transcript;
pub mod harness;
pub mod opencode_session;
pub mod plugin_shadow;

pub use harness::{
    ClaudeCodeAdapter, CliDispatchContext, CliJudgeContext, CliManifestContext, CodexAdapter,
    HEADLESS_RUNBOOK_TEMPLATE, HarnessAdapter, OpenCodeAdapter, adapter_for,
};

pub use claude_code_session::{render_available_skills_block, render_plan_mode_context};
pub use claude_code_transcript::TranscriptSummary;
pub use claude_stream_json::{parse_claude_stream_json, parse_claude_stream_json_full};
pub use codex_session::{render_codex_available_skills_block, render_codex_plan_mode_context};
pub use codex_transcript::{parse_codex_events, parse_codex_events_full};
pub use opencode_session::{
    render_opencode_available_skills_block, render_opencode_plan_mode_context,
};
pub use plugin_shadow::{
    PluginShadowReport, ShadowSource, config_dir_from_env, detect_plugin_shadows,
    format_shadow_banner, resolve_config_dir, shadow_validity_warnings,
};
