//! Execution sandbox: write-guard install/teardown and write-boundary policy.
//!
//! Mirrors `src/sandbox/` in eval-runner (`sandbox-policy.ts`, `policy.ts`,
//! `install.ts`, `guard.ts`). The Node hook script `guard.ts` becomes the hidden
//! `guard` subcommand on this binary (see [`guard`] and `cli`), so the installed
//! PreToolUse hook invokes `skill-eval guard <marker>` instead of a script path.

pub mod decide;
pub mod guard;
pub mod install;
pub mod policy;

pub use decide::{GuardDecision, GuardMarker, decide};
pub use guard::{guard_decision, read_marker};
pub use install::{GUARD_MANIFEST, GUARD_MARKER, install_guard, teardown_guard};
pub use policy::{WRITE_TOOLS, classify_bash, is_under, is_under_any, is_write_tool, path_arg};

use std::time::{SystemTime, UNIX_EPOCH};

/// Current wall clock in epoch milliseconds. chrono ships without its `clock`
/// feature (it parses timestamps but never reads the clock), so the time comes
/// from `std::time`. Shared by the guard's expiry check and marker stamping.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
