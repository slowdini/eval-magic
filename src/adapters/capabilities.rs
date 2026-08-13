//! Named code capabilities a harness descriptor references.
//!
//! Everything a descriptor cannot express as data — transcript stitching or
//! harness-specific denial evidence, slug sanitization, plugin-shadow scanning
//! — lives behind one of these closed enums. A descriptor opts in by naming the
//! capability (`parser = "codex-items"` or
//! `permission_denials_parser = "codex-items"`); a harness whose stream is
//! compatible with an existing capability gets the feature from configuration
//! alone. (The write guard needs no named capability: its install and verdict
//! render from the descriptor's `[guard]` data via [`super::guard`].)
//!
//! The enums deserialize from the kebab-case capability names the
//! `harness-descriptor` schema also enumerates, so an unknown name fails the
//! schema gate with a listed-allowed-values message before ever reaching Rust.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::ToolInvocation;

use super::skill_shadow::{PluginShadowReport, ShadowSource};
use super::{PermissionDenial, TranscriptSummary};

/// Code-backed transcript readers. Every variant can produce a primary summary
/// when selected as `parser`; descriptors may also select its independent
/// denial reader or rely on its legacy session-surface support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TranscriptParser {
    /// `claude -p --output-format stream-json` events.
    ClaudeStreamJson,
    /// `cline --json` `agent_event`/`run_result` NDJSON.
    ClineJson,
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
            TranscriptParser::ClineJson => super::cline::transcript::parse_cline_events(path),
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
            TranscriptParser::ClineJson => super::cline::transcript::parse_cline_events_full(path),
            TranscriptParser::CodexItems => super::codex::transcript::parse_codex_events_full(path),
            TranscriptParser::OpencodeEvents => {
                super::opencode::transcript::parse_opencode_events_full(path)
            }
        }
    }

    /// Whether this reader can identify tool calls the harness refused to run.
    /// Refusals are encoded differently by every CLI, so detection is opt-in per
    /// reader; the rest report none rather than guessing from result text.
    pub(crate) fn surfaces_permission_denials(self) -> bool {
        matches!(
            self,
            TranscriptParser::ClaudeStreamJson
                | TranscriptParser::ClineJson
                | TranscriptParser::CodexItems
                | TranscriptParser::OpencodeEvents
        )
    }

    /// Parse refused tool calls associated with the captured events path. A
    /// reader may correlate sibling captures. Empty for readers that don't
    /// surface denials — see
    /// [`surfaces_permission_denials`](Self::surfaces_permission_denials).
    pub(crate) fn parse_permission_denials(self, path: &Path) -> io::Result<Vec<PermissionDenial>> {
        match self {
            TranscriptParser::ClaudeStreamJson => {
                super::claude_code::stream_json::parse_claude_permission_denials(path)
            }
            TranscriptParser::ClineJson => {
                super::cline::transcript::parse_cline_permission_denials(path)
            }
            TranscriptParser::CodexItems => {
                super::codex::transcript::parse_codex_permission_denials(path)
            }
            TranscriptParser::OpencodeEvents => {
                super::opencode::transcript::parse_opencode_permission_denials(path)
            }
        }
    }

    /// Whether this parser's event stream has legacy support for the session's
    /// discoverable skills and loaded plugins. New descriptors should prefer
    /// the generic `transcript.extract.session_surface` mapping.
    pub(crate) fn surfaces_session_surface(self) -> bool {
        matches!(self, TranscriptParser::ClaudeStreamJson)
    }

    /// Parse the session's skill/plugin surface. `None` for parsers whose streams
    /// carry no roster — see
    /// [`surfaces_session_surface`](Self::surfaces_session_surface).
    pub(crate) fn parse_session_surface(
        self,
        path: &Path,
    ) -> io::Result<Option<super::SessionSurface>> {
        match self {
            TranscriptParser::ClaudeStreamJson => {
                super::claude_code::stream_json::parse_claude_session_surface(path)
            }
            TranscriptParser::ClineJson
            | TranscriptParser::CodexItems
            | TranscriptParser::OpencodeEvents => Ok(None),
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

    /// Resolve duplicate runtime ids using the harness's declared preflight
    /// capability. Report grouping and severity remain harness-neutral.
    pub(crate) fn resolve(self, scan_root: &Path, sources: &mut [ShadowSource]) {
        match self {
            ShadowPreflight::ClaudePlugins => super::skill_shadow::resolve_by_precedence(sources),
            ShadowPreflight::CodexSkills => super::skill_shadow::resolve_as_coexisting(sources),
            ShadowPreflight::OpencodeSkills => {
                super::opencode::skill_shadow::resolve_sources(scan_root, sources)
            }
        }
    }
}
