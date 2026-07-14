//! Named code capabilities a harness descriptor references.
//!
//! Everything a descriptor cannot express as data — transcript stitching,
//! guard hook installation, slug sanitization, plugin-shadow scanning — lives
//! behind one of these closed enums. A descriptor opts in by naming the
//! capability (`parser = "codex-items"`, `engine = "claude-hooks"`); a harness
//! whose stream or hooks are compatible with an existing capability gets the
//! full feature from configuration alone.
//!
//! The enums deserialize from the kebab-case capability names the
//! `harness-descriptor` schema also enumerates, so an unknown name fails the
//! schema gate with a listed-allowed-values message before ever reaching Rust.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
}

impl TranscriptParser {
    /// Parse the captured events file into ordered tool invocations.
    pub(crate) fn parse(self, path: &Path) -> io::Result<Vec<ToolInvocation>> {
        match self {
            TranscriptParser::ClaudeStreamJson => {
                super::claude_code::stream_json::parse_claude_stream_json(path)
            }
            TranscriptParser::CodexItems => super::codex::transcript::parse_codex_events(path),
        }
    }

    /// The full-summary counterpart of [`parse`](Self::parse).
    pub(crate) fn parse_full(self, path: &Path) -> io::Result<TranscriptSummary> {
        match self {
            TranscriptParser::ClaudeStreamJson => {
                super::claude_code::stream_json::parse_claude_stream_json_full(path)
            }
            TranscriptParser::CodexItems => super::codex::transcript::parse_codex_events_full(path),
        }
    }
}

/// Write-guard engines: install a PreToolUse-style hook under the staged env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GuardEngine {
    /// Hook merged into `.claude/settings.local.json`.
    ClaudeHooks,
    /// Hook merged into `.codex/hooks.json`.
    CodexHooks,
}

impl GuardEngine {
    /// The tool-name matcher the engine's hook registers for. Exposed so
    /// descriptor validation can prove every hooked tool is declared in the
    /// descriptor's `[tools]` vocabulary.
    pub(crate) fn hook_matcher(self) -> &'static str {
        match self {
            GuardEngine::ClaudeHooks => super::claude_code::guard::HOOK_MATCHER,
            GuardEngine::CodexHooks => super::codex::guard::HOOK_MATCHER,
        }
    }

    /// Arm the write guard under `stage_root`, returning the staged marker
    /// path.
    pub(crate) fn install_guard(
        self,
        stage_root: &Path,
        guard_exe: &Path,
        ttl: Option<Duration>,
    ) -> io::Result<PathBuf> {
        match self {
            GuardEngine::ClaudeHooks => {
                super::claude_code::guard::install_guard(stage_root, guard_exe, ttl)
            }
            GuardEngine::CodexHooks => {
                super::codex::guard::install_guard(stage_root, guard_exe, ttl)
            }
        }
    }

    /// A hook-config dir the install created outside the skills dir, which
    /// teardown prunes when restoring the original config leaves it empty.
    pub(crate) fn hook_cleanup_dir(self, stage_root: &Path) -> Option<PathBuf> {
        match self {
            // The Claude hook lives in .claude/, which staging already owns.
            GuardEngine::ClaudeHooks => None,
            GuardEngine::CodexHooks => Some(super::codex::guard::hook_cleanup_dir(stage_root)),
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

/// Shadow preflights: detect installed skills that shadow a staged slug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowPreflight {
    /// Claude Code plugin/skill scan rooted at the user config dir.
    ClaudePlugins,
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
        }
    }
}
