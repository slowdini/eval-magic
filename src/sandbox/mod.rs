//! Execution sandbox: write-guard install/teardown and write-boundary policy.
//!
//! The hook entry points are hidden subcommands on this binary (see [`guard`] and
//! `cli`), so the installed PreToolUse hook invokes `eval-magic guard <marker>`
//! or `eval-magic guard-codex <marker>` — no separate hook script to ship or
//! locate.

pub mod decide;
pub mod guard;
pub mod install;
pub mod policy;

pub use decide::{GuardDecision, GuardMarker, decide};
pub use guard::{codex_guard_decision, guard_decision, read_marker};
pub use install::{
    GUARD_MANIFEST, GUARD_MARKER, install_guard, install_guard_for_harness, teardown_guard,
};
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
