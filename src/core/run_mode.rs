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

/// The dispatch mechanism a harness uses today. This is the single, documented
/// place where the current 1:1 harness↔mechanism coupling lives — when a harness
/// gains a second mechanism (e.g. a true headless Claude Code mode), the choice
/// stops being derivable from the harness alone and this is the seam that grows
/// to take an explicit selection.
pub fn mechanism_for(harness: Harness) -> DispatchMechanism {
    match harness {
        Harness::ClaudeCode => DispatchMechanism::InSession,
        Harness::Codex | Harness::OpenCode => DispatchMechanism::Cli,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_harness_to_its_mechanism_today() {
        assert_eq!(
            mechanism_for(Harness::ClaudeCode),
            DispatchMechanism::InSession
        );
        assert_eq!(mechanism_for(Harness::Codex), DispatchMechanism::Cli);
        assert_eq!(mechanism_for(Harness::OpenCode), DispatchMechanism::Cli);
    }
}
