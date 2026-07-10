//! The harness adapter layer.
//!
//! [`harness`] defines the [`HarnessAdapter`] trait — the single API generic
//! dispatch code uses to reach harness-specific behavior. Everything specific
//! to one harness lives in that harness's module tree ([`claude_code`],
//! [`codex`], [`opencode`]): the adapter impl, session renderers, transcript
//! parsers, dispatch-recipe rendering, and write-guard hooks. Generic code
//! resolves an adapter with [`adapter_for`] and calls the trait.

pub mod claude_code;
mod cli_command;
pub mod codex;
pub mod harness;
pub mod opencode;
pub mod skill_shadow;
pub mod transcript;

pub use harness::{
    CliDispatchContext, CliJudgeContext, CliManifestContext, HarnessAdapter, RUNBOOK_TEMPLATE,
    adapter_for, all_config_dir_names,
};
pub use skill_shadow::{
    PluginShadowReport, ShadowSource, format_shadow_banner, shadow_validity_warnings,
};
pub use transcript::TranscriptSummary;
