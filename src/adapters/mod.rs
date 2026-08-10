//! The harness adapter layer.
//!
//! [`harness`] defines the [`HarnessAdapter`] trait — the single API generic
//! dispatch code uses to reach harness-specific behavior. Each harness's
//! declarative half lives in its embedded descriptor file
//! (`harnesses/<label>.toml`, loaded by [`descriptor`] and served through the
//! generic [`descriptor_adapter`]); the code-backed features a descriptor
//! references by name live in [`capabilities`], backed by the per-harness
//! module trees ([`claude_code`], [`codex`], [`opencode`]): transcript
//! summary/denial readers, plugin-shadow detection, slug sanitization. The
//! write guard is pure descriptor data rendered by the generic engine in
//! `guard`.
//! The [`registry`] loads the descriptors into label-keyed entries and owns
//! harness-identifier resolution; generic code resolves an adapter with
//! [`adapter_for`] and calls the trait.
//!
//! The resulting two-way reference with `crate::sandbox` is intentional. This
//! module owns the harness-facing guard contract: descriptor fields, native
//! hook surfaces, and verdict rendering. `sandbox` owns harness-neutral
//! enforcement: marker/manifest lifecycle, boundary classification, the
//! arbiter, and cross-harness cleanup. Put new code here when it describes how
//! a harness integrates with the guard; put it in `sandbox` when it enforces
//! the boundary independently of any one harness.

pub mod capabilities;
pub mod claude_code;
pub(crate) mod cli_command;
pub mod codex;
pub mod descriptor;
pub mod descriptor_adapter;
pub mod extract;
pub(crate) mod guard;
pub mod harness;
pub mod opencode;
pub mod registry;
pub mod skill_shadow;
mod skills_block;
pub mod transcript;

pub use harness::{
    CliDispatchContext, CliJudgeContext, CliManifestContext, HarnessAdapter, RUNBOOK_TEMPLATE,
    TokenUsageAggregation, ToolVocabulary,
};
pub use registry::{
    DEFAULT_HARNESS_NAME, UnknownHarnessError, adapter_for, all_config_dir_names,
    all_tool_vocabulary,
};
pub use skill_shadow::{
    PluginShadowReport, ShadowAppearance, ShadowFinding, ShadowNamespace, ShadowRelation,
    ShadowResolution, ShadowRoot, ShadowRootScope, ShadowSeverity, ShadowSkillRole, ShadowSource,
    ShadowSourceKind, ShadowSourceOrigin, format_shadow_banner, shadow_validity_warnings,
};
pub use transcript::{LoadedPlugin, PermissionDenial, SessionSurface, TranscriptSummary};
