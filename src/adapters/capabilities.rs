//! Named code capabilities a harness descriptor references.
//!
//! Everything a descriptor cannot express as data — transcript stitching,
//! slug sanitization, plugin-shadow scanning — lives behind one of these
//! closed enums. A descriptor opts in by naming the capability
//! (`parser = "codex-items"`); a harness whose stream is compatible with an
//! existing capability gets the full feature from configuration alone. (The
//! write guard needs no named capability: its install and verdict render from
//! the descriptor's `[guard]` data via [`super::guard`].)
//!
//! The enums deserialize from the kebab-case capability names the
//! `harness-descriptor` schema also enumerates, so an unknown name fails the
//! schema gate with a listed-allowed-values message before ever reaching Rust.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::ToolInvocation;

use super::TranscriptSummary;
use super::skill_shadow::PluginShadowReport;

/// Transcript parsers: turn a captured CLI events file into tool invocations
/// and a [`super::TranscriptSummary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TranscriptParser {
    /// `claude -p --output-format stream-json` events.
    ClaudeStreamJson,
    /// `codex exec --json` `item.completed` events.
    CodexItems,
    /// `opencode run --format json` `tool_use`/`text`/`step_finish` events.
    OpencodeEvents,
}

impl TranscriptParser {
    /// Parse the captured events file into ordered tool invocations.
    pub(crate) fn parse(self, path: &Path) -> io::Result<Vec<ToolInvocation>> {
        match self {
            TranscriptParser::ClaudeStreamJson => {
                super::claude_code::stream_json::parse_claude_stream_json(path)
            }
            TranscriptParser::CodexItems => super::codex::transcript::parse_codex_events(path),
            TranscriptParser::OpencodeEvents => {
                super::opencode::transcript::parse_opencode_events(path)
            }
        }
    }

    /// The full-summary counterpart of [`parse`](Self::parse).
    pub(crate) fn parse_full(self, path: &Path) -> io::Result<TranscriptSummary> {
        match self {
            TranscriptParser::ClaudeStreamJson => {
                super::claude_code::stream_json::parse_claude_stream_json_full(path)
            }
            TranscriptParser::CodexItems => super::codex::transcript::parse_codex_events_full(path),
            TranscriptParser::OpencodeEvents => {
                super::opencode::transcript::parse_opencode_events_full(path)
            }
        }
    }
}

/// Staged-slug generators, for harnesses whose naming rules need
/// sanitization/truncation beyond a format string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SlugCapability {
    /// OpenCode's lowercase-alphanumeric-single-hyphen names, length-capped.
    Opencode,
}

impl SlugCapability {
    /// Generate the staged slug for one `(iteration, condition, skill)` cell.
    pub(crate) fn staged_slug(
        self,
        prefix: &str,
        iteration: u32,
        condition: &str,
        skill_name: &str,
    ) -> String {
        match self {
            SlugCapability::Opencode => {
                super::opencode::opencode_slug(prefix, iteration, condition, skill_name)
            }
        }
    }
}

/// Shadow preflights: detect live skills that shadow a logical staged skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowPreflight {
    /// Claude Code plugin/skill scan rooted at the user config dir.
    ClaudePlugins,
    /// Codex repo/user/admin/plugin skill scan.
    CodexSkills,
    /// OpenCode project/global `.opencode`/`.claude`/`.agents` skill scan.
    OpencodeSkills,
}

impl ShadowPreflight {
    /// Detect staged skill names that are also discoverable from the
    /// operator's live environment. `None` when nothing is shadowed.
    pub(crate) fn detect(
        self,
        scan_root: &Path,
        staged_skill_names: &[&str],
    ) -> Option<PluginShadowReport> {
        match self {
            ShadowPreflight::ClaudePlugins => super::claude_code::plugin_shadow::shadow_preflight(
                &super::claude_code::plugin_shadow::config_dir_from_env(),
                scan_root,
                staged_skill_names,
            ),
            ShadowPreflight::CodexSkills => {
                super::codex::skill_shadow::shadow_preflight(scan_root, staged_skill_names)
            }
            ShadowPreflight::OpencodeSkills => {
                super::opencode::skill_shadow::shadow_preflight(scan_root, staged_skill_names)
            }
        }
    }

    /// Render the harness-specific build-time warning for a shadow report.
    pub(crate) fn format_banner(self, report: &PluginShadowReport) -> String {
        match self {
            ShadowPreflight::ClaudePlugins => super::skill_shadow::format_shadow_banner(report),
            ShadowPreflight::CodexSkills => {
                super::codex::skill_shadow::format_shadow_banner(report)
            }
            ShadowPreflight::OpencodeSkills => {
                super::opencode::skill_shadow::format_shadow_banner(report)
            }
        }
    }

    /// Render harness-specific aggregate validity warnings for a report.
    pub(crate) fn validity_warnings(self, report: &PluginShadowReport) -> Vec<String> {
        match self {
            ShadowPreflight::ClaudePlugins => super::skill_shadow::shadow_validity_warnings(report),
            ShadowPreflight::CodexSkills => {
                super::codex::skill_shadow::shadow_validity_warnings(report)
            }
            ShadowPreflight::OpencodeSkills => {
                super::opencode::skill_shadow::shadow_validity_warnings(report)
            }
        }
    }
}
