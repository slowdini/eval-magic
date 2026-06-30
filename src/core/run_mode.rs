//! Run mode — *how* an eval is dispatched, independent of *which* harness runs
//! it.
//!
//! Every dispatch now rides a single mechanism: each task is delivered through a
//! one-shot harness CLI subprocess (`claude -p`, `codex exec`). The two
//! *user-facing* run modes documented in the README — **hybrid** and
//! **headless** — share that mechanism and differ only in whether an agent or a
//! human session drives the loop, not in how a single task reaches the harness.
//! (The vocabulary collapse that folds these two into one is tracked separately.)
//!
//! This is distinct from the comparison [`Mode`](crate::core::Mode)
//! (`new-skill` / `revision`), which selects the two conditions being compared,
//! not the dispatch path.

use serde::{Deserialize, Serialize};

use crate::core::Harness;

/// The user-facing run mode — *who/what drives the loop*. Both modes dispatch
/// each task through the harness CLI; they differ only in whether an agent
/// session (`hybrid`) or a human (`headless`) drives the loop — a distinction we
/// persist (in `conditions.json`) even though it doesn't change how a single task
/// reaches the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum RunMode {
    /// An agent session orchestrates while each dispatch shells out to the
    /// harness CLI (`claude -p`, `codex exec`).
    Hybrid,
    /// No session drives the loop; eval-magic commands dispatch through the
    /// harness CLI end to end.
    Headless,
}

impl RunMode {
    /// The default run mode for a harness when `--run-mode` is omitted. Every
    /// harness defaults to `hybrid`: an agent session drives the loop and each
    /// dispatch shells out to the harness CLI.
    pub fn default_for(_harness: Harness) -> RunMode {
        RunMode::Hybrid
    }

    /// The kebab-case identifier (matches the `--run-mode` flag values and the
    /// serialized form in `conditions.json`).
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::Hybrid => "hybrid",
            RunMode::Headless => "headless",
        }
    }
}

/// Resolve the effective run mode for a harness, defaulting per harness when
/// unspecified and rejecting unsupported `(harness, mode)` combinations. The
/// `Err` string is operator-facing.
pub fn resolve_run_mode(harness: Harness, requested: Option<RunMode>) -> Result<RunMode, String> {
    let mode = requested.unwrap_or_else(|| RunMode::default_for(harness));
    let supported: &[RunMode] = match harness {
        // Claude Code and Codex both wire the CLI mechanism, so both modes apply
        // (hybrid is agent-driven, headless human-driven).
        Harness::ClaudeCode | Harness::Codex => &[RunMode::Hybrid, RunMode::Headless],
        // OpenCode's CLI path is only partially wired (no transcript ingest), so
        // only hybrid is advertised for now.
        Harness::OpenCode => &[RunMode::Hybrid],
    };
    if supported.contains(&mode) {
        return Ok(mode);
    }
    let supported_list = supported
        .iter()
        .map(|m| m.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "--run-mode {} is not supported for --harness {}; supported: {}",
        mode.as_str(),
        harness_label(harness),
        supported_list,
    ))
}

/// The kebab-case CLI identifier for a harness (for operator-facing messages).
fn harness_label(harness: Harness) -> &'static str {
    match harness {
        Harness::ClaudeCode => "claude-code",
        Harness::Codex => "codex",
        Harness::OpenCode => "opencode",
    }
}

/// Run-option support for a harness's currently wired dispatch mechanism.
///
/// This is intentionally narrower than full harness parity: it only describes
/// options the generic `run` preflight must accept or reject before the build
/// sequence starts. Harness-specific behavior still lives behind the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessRunCapabilities {
    pub supports_guard: bool,
    pub supports_bootstrap_with_no_stage: bool,
    pub supports_stage_name_with_no_stage: bool,
}

/// The focused capability table for generic `run` option validation.
pub fn capabilities_for(harness: Harness) -> HarnessRunCapabilities {
    match harness {
        Harness::ClaudeCode => HarnessRunCapabilities {
            supports_guard: true,
            supports_bootstrap_with_no_stage: true,
            supports_stage_name_with_no_stage: true,
        },
        Harness::Codex => HarnessRunCapabilities {
            supports_guard: true,
            supports_bootstrap_with_no_stage: false,
            supports_stage_name_with_no_stage: false,
        },
        Harness::OpenCode => HarnessRunCapabilities {
            supports_guard: false,
            supports_bootstrap_with_no_stage: true,
            supports_stage_name_with_no_stage: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_capture_run_option_support_by_harness() {
        let claude = capabilities_for(Harness::ClaudeCode);
        assert!(claude.supports_guard);
        assert!(claude.supports_bootstrap_with_no_stage);
        assert!(claude.supports_stage_name_with_no_stage);

        let codex = capabilities_for(Harness::Codex);
        assert!(codex.supports_guard);
        assert!(!codex.supports_bootstrap_with_no_stage);
        assert!(!codex.supports_stage_name_with_no_stage);

        let opencode = capabilities_for(Harness::OpenCode);
        assert!(!opencode.supports_guard);
        assert!(opencode.supports_bootstrap_with_no_stage);
        assert!(opencode.supports_stage_name_with_no_stage);
    }

    #[test]
    fn run_mode_defaults_to_hybrid_for_every_harness() {
        assert_eq!(RunMode::default_for(Harness::ClaudeCode), RunMode::Hybrid);
        assert_eq!(RunMode::default_for(Harness::Codex), RunMode::Hybrid);
        assert_eq!(RunMode::default_for(Harness::OpenCode), RunMode::Hybrid);
    }

    #[test]
    fn resolve_run_mode_defaults_when_unspecified() {
        assert_eq!(
            resolve_run_mode(Harness::ClaudeCode, None).unwrap(),
            RunMode::Hybrid
        );
        assert_eq!(
            resolve_run_mode(Harness::Codex, None).unwrap(),
            RunMode::Hybrid
        );
    }

    #[test]
    fn resolve_run_mode_accepts_claude_hybrid() {
        assert_eq!(
            resolve_run_mode(Harness::ClaudeCode, Some(RunMode::Hybrid)).unwrap(),
            RunMode::Hybrid
        );
    }

    #[test]
    fn resolve_run_mode_rejects_headless_for_opencode() {
        let err = resolve_run_mode(Harness::OpenCode, Some(RunMode::Headless)).unwrap_err();
        assert!(err.contains("headless"), "message was: {err}");
        assert!(err.contains("opencode"), "message was: {err}");
    }

    #[test]
    fn resolve_run_mode_accepts_claude_headless() {
        assert_eq!(
            resolve_run_mode(Harness::ClaudeCode, Some(RunMode::Headless)).unwrap(),
            RunMode::Headless
        );
    }

    #[test]
    fn resolve_run_mode_accepts_codex_headless() {
        assert_eq!(
            resolve_run_mode(Harness::Codex, Some(RunMode::Headless)).unwrap(),
            RunMode::Headless
        );
    }

    #[test]
    fn run_mode_serde_roundtrips_kebab_case() {
        assert_eq!(
            serde_json::to_string(&RunMode::Hybrid).unwrap(),
            "\"hybrid\""
        );
        let parsed: RunMode = serde_json::from_str("\"headless\"").unwrap();
        assert_eq!(parsed, RunMode::Headless);
    }
}
