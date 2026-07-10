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

use serde::Deserialize;

/// Transcript parsers: turn a captured CLI events file into tool invocations
/// and a [`super::TranscriptSummary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TranscriptParser {
    /// `claude -p --output-format stream-json` events.
    ClaudeStreamJson,
    /// `codex exec --json` `item.completed` events.
    CodexItems,
}

/// Write-guard engines: install a PreToolUse-style hook under the staged env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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
}

/// Staged-slug generators, for harnesses whose naming rules need
/// sanitization/truncation beyond a format string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowPreflight {
    /// Claude Code plugin/skill scan rooted at the user config dir.
    ClaudePlugins,
}
