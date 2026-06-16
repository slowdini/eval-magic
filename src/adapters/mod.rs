//! Harness-specific session rendering and transcript parsing.
//!
//! Claude Code and Codex session renderers + transcript parsers, plus
//! plugin-shadow detection. The submodules are re-exported flat so downstream
//! code writes `crate::adapters::<fn>`.

pub mod claude_code_session;
pub mod claude_code_transcript;
pub mod codex_session;
pub mod codex_transcript;
pub mod opencode_session;
pub mod plugin_shadow;

pub use claude_code_session::{
    render_available_skills_block, render_plan_mode_context, resolve_subagents_dir_for_session,
    slugify_project_path,
};
pub use claude_code_transcript::{
    SubagentEntry, SubagentMeta, TranscriptSummary, find_by_description, list_subagents,
    parse_transcript, parse_transcript_full,
};
pub use codex_session::{render_codex_available_skills_block, render_codex_plan_mode_context};
pub use codex_transcript::{parse_codex_events, parse_codex_events_full};
pub use opencode_session::{
    render_opencode_available_skills_block, render_opencode_plan_mode_context,
};
pub use plugin_shadow::{
    PluginShadowReport, ShadowSource, config_dir_from_env, detect_plugin_shadows,
    format_shadow_banner, resolve_config_dir, shadow_validity_warnings,
};
