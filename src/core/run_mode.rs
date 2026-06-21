//! Run mode — *how* an eval is dispatched, independent of *which* harness runs
//! it.
//!
//! There are two dispatch **mechanisms** in the code today:
//!
//! - [`DispatchMechanism::InSession`] — the runner hands tasks to in-session
//!   subagents (Claude Code's Task tool). The reference is Claude Code.
//! - [`DispatchMechanism::Cli`] — each task is dispatched through a one-shot
//!   harness CLI subprocess (`codex exec`). The reference is Codex.
//!
//! These two mechanisms underpin the three *user-facing* run modes documented in
//! the README: **fully-interactive** rides on [`InSession`](DispatchMechanism::InSession);
//! **headless** and **hybrid** both ride on [`Cli`](DispatchMechanism::Cli),
//! differing only in whether a human/agent session drives the loop — not in how
//! a single task reaches the harness.
//!
//! This is distinct from the comparison [`Mode`](crate::core::Mode)
//! (`new-skill` / `revision`), which selects the two conditions being compared,
//! not the dispatch path.

use serde::{Deserialize, Serialize};

use crate::core::Harness;

/// How a single dispatch is delivered to a harness. The primary code axis for
/// run-mode concerns (next-steps guidance, transcript source).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchMechanism {
    /// In-session subagent dispatch (Claude Code's Task tool).
    InSession,
    /// One-shot harness CLI subprocess dispatch (`codex exec`).
    Cli,
}

/// The user-facing run mode — *who/what drives the loop* plus which dispatch
/// mechanism each task rides on. This is the parity vocabulary documented in the
/// README (§Run modes); it maps down to a [`DispatchMechanism`] via
/// [`RunMode::mechanism`]. `hybrid` and `headless` both ride on
/// [`Cli`](DispatchMechanism::Cli) and differ only in whether a session drives
/// the loop — a distinction we persist (in `conditions.json`) even though it
/// doesn't change how a single task reaches the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum RunMode {
    /// In-session subagent dispatch (Claude Code's Task tool).
    Interactive,
    /// An agent session orchestrates while each dispatch shells out to the
    /// harness CLI (`claude -p`, `codex exec`).
    Hybrid,
    /// No session drives the loop; eval-magic commands dispatch through the
    /// harness CLI end to end.
    Headless,
}

impl RunMode {
    /// The dispatch mechanism this run mode rides on.
    pub fn mechanism(self) -> DispatchMechanism {
        match self {
            RunMode::Interactive => DispatchMechanism::InSession,
            RunMode::Hybrid | RunMode::Headless => DispatchMechanism::Cli,
        }
    }

    /// The default run mode for a harness when `--run-mode` is omitted, chosen to
    /// preserve today's behavior: Claude Code → interactive, the CLI-dispatch
    /// harnesses → hybrid.
    pub fn default_for(harness: Harness) -> RunMode {
        match harness {
            Harness::ClaudeCode => RunMode::Interactive,
            Harness::Codex | Harness::OpenCode => RunMode::Hybrid,
        }
    }

    /// The kebab-case identifier (matches the `--run-mode` flag values and the
    /// serialized form in `conditions.json`).
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::Interactive => "interactive",
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
        // Claude Code wires every mode: in-session (interactive) plus both CLI
        // modes (hybrid and headless ride the same `claude -p` mechanism).
        Harness::ClaudeCode => &[RunMode::Interactive, RunMode::Hybrid, RunMode::Headless],
        // Codex dispatches via subprocess, so in-session doesn't translate, but
        // both CLI modes do (hybrid is agent-driven, headless human-driven).
        Harness::Codex => &[RunMode::Hybrid, RunMode::Headless],
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
    pub mechanism: DispatchMechanism,
    pub supports_guard: bool,
    pub supports_bootstrap_with_no_stage: bool,
    pub supports_stage_name_with_no_stage: bool,
}

/// The focused capability table for generic `run` option validation.
pub fn capabilities_for(harness: Harness) -> HarnessRunCapabilities {
    match harness {
        Harness::ClaudeCode => HarnessRunCapabilities {
            mechanism: DispatchMechanism::InSession,
            supports_guard: true,
            supports_bootstrap_with_no_stage: true,
            supports_stage_name_with_no_stage: true,
        },
        Harness::Codex => HarnessRunCapabilities {
            mechanism: DispatchMechanism::Cli,
            supports_guard: true,
            supports_bootstrap_with_no_stage: false,
            supports_stage_name_with_no_stage: false,
        },
        Harness::OpenCode => HarnessRunCapabilities {
            mechanism: DispatchMechanism::Cli,
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
        assert_eq!(claude.mechanism, DispatchMechanism::InSession);
        assert!(claude.supports_guard);
        assert!(claude.supports_bootstrap_with_no_stage);
        assert!(claude.supports_stage_name_with_no_stage);

        let codex = capabilities_for(Harness::Codex);
        assert_eq!(codex.mechanism, DispatchMechanism::Cli);
        assert!(codex.supports_guard);
        assert!(!codex.supports_bootstrap_with_no_stage);
        assert!(!codex.supports_stage_name_with_no_stage);

        let opencode = capabilities_for(Harness::OpenCode);
        assert_eq!(opencode.mechanism, DispatchMechanism::Cli);
        assert!(!opencode.supports_guard);
        assert!(opencode.supports_bootstrap_with_no_stage);
        assert!(opencode.supports_stage_name_with_no_stage);
    }

    #[test]
    fn run_mode_mechanism_maps_each_mode() {
        assert_eq!(
            RunMode::Interactive.mechanism(),
            DispatchMechanism::InSession
        );
        assert_eq!(RunMode::Hybrid.mechanism(), DispatchMechanism::Cli);
        assert_eq!(RunMode::Headless.mechanism(), DispatchMechanism::Cli);
    }

    #[test]
    fn run_mode_default_per_harness_preserves_today() {
        assert_eq!(
            RunMode::default_for(Harness::ClaudeCode),
            RunMode::Interactive
        );
        assert_eq!(RunMode::default_for(Harness::Codex), RunMode::Hybrid);
        assert_eq!(RunMode::default_for(Harness::OpenCode), RunMode::Hybrid);
    }

    #[test]
    fn resolve_run_mode_defaults_when_unspecified() {
        assert_eq!(
            resolve_run_mode(Harness::ClaudeCode, None).unwrap(),
            RunMode::Interactive
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
    fn resolve_run_mode_rejects_interactive_for_cli_harnesses() {
        let err = resolve_run_mode(Harness::Codex, Some(RunMode::Interactive)).unwrap_err();
        assert!(err.contains("interactive"), "message was: {err}");
        assert!(err.contains("codex"), "message was: {err}");
        assert!(resolve_run_mode(Harness::OpenCode, Some(RunMode::Interactive)).is_err());
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
