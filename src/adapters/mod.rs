//! The harness adapter layer.
//!
//! [`harness`] defines the [`HarnessAdapter`] trait — the single API generic
//! dispatch code uses to reach harness-specific behavior. Each harness's
//! declarative half lives in its embedded descriptor file
//! (`harnesses/<label>.toml`, loaded by [`descriptor`] and served through the
//! generic [`descriptor_adapter`]); the code-backed features a descriptor
//! references by name live in [`capabilities`], backed by the per-harness
//! module trees ([`claude_code`], [`codex`], [`opencode`]): transcript
//! parsers, write-guard hooks, plugin-shadow detection, slug sanitization.
//! Generic code resolves an adapter with [`adapter_for`] and calls the trait.

pub mod capabilities;
pub mod claude_code;
mod cli_command;
pub mod codex;
pub mod descriptor;
pub mod descriptor_adapter;
pub mod harness;
pub mod opencode;
pub mod skill_shadow;
mod skills_block;
pub mod transcript;

pub use harness::{
    CliDispatchContext, CliJudgeContext, CliManifestContext, DEFAULT_HARNESS_NAME, HarnessAdapter,
    RUNBOOK_TEMPLATE, ToolVocabulary, UnknownHarnessError, adapter_for, all_config_dir_names,
    all_tool_vocabulary, harness_value_parser,
};
pub use skill_shadow::{
    PluginShadowReport, ShadowSource, format_shadow_banner, shadow_validity_warnings,
};
pub use transcript::TranscriptSummary;
