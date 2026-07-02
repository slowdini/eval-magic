//! Per-harness run-option capabilities.
//!
//! Every dispatch rides a single mechanism: each task is delivered through a
//! one-shot harness CLI subprocess (`claude -p`, `codex exec`). What still varies
//! by harness is which *run options* the generic `run` preflight may accept —
//! captured here as a narrow capability table, independent of the comparison
//! [`Mode`](crate::core::Mode) (`new-skill` / `revision`), which selects the two
//! conditions being compared.

use crate::core::Harness;

/// Run-option support for a harness's dispatch path.
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
}
