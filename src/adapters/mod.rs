//! Harness-specific session rendering and transcript parsing.
//!
//! Claude Code and Codex session renderers + transcript parsers, plus
//! plugin-shadow detection. The submodules are re-exported flat so downstream
//! code writes `crate::adapters::<fn>`.

pub mod claude_code_session;
pub mod claude_code_transcript;
pub mod codex_session;
pub mod codex_transcript;
pub mod plugin_shadow;

pub use claude_code_session::{render_available_skills_block, render_plan_mode_context};
pub use claude_code_transcript::{
    SubagentEntry, SubagentMeta, TranscriptSummary, find_by_description, list_subagents,
    parse_transcript, parse_transcript_full,
};
pub use codex_session::render_codex_available_skills_block;
pub use codex_transcript::{parse_codex_events, parse_codex_events_full};
pub use plugin_shadow::{
    PluginShadowReport, ShadowSource, detect_plugin_shadows, format_shadow_banner,
    resolve_config_dir, shadow_validity_warnings,
};
