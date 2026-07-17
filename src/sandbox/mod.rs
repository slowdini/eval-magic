//! Execution sandbox: shared write-guard machinery and write-boundary policy.
//!
//! The hook entry points are hidden subcommands on this binary (see `cli`), so
//! the installed PreToolUse hook invokes `eval-magic guard <marker>`,
//! `eval-magic guard-codex <marker>`, or the generic
//! `eval-magic guard-hook --harness <name> <marker>` — no separate hook script
//! to ship or locate. Each harness's hook path, matcher, and verdict shape are
//! descriptor data rendered by the generic engine (`crate::adapters::guard`);
//! this module holds the shared marker/manifest/teardown machinery and the
//! boundary policy.

pub mod decide;
pub mod guard;
pub mod install;
pub mod policy;

pub(crate) use decide::marker_is_armed;
pub use decide::{GuardDecision, GuardMarker, decide};
pub(crate) use guard::parse_tool_call;
pub use guard::read_marker;
pub(crate) use install::guard_is_armed;
pub use install::{GUARD_MANIFEST, GUARD_MARKER, teardown_guard};
pub use policy::{classify_bash, is_shell_tool, is_under, is_under_any, is_write_tool, path_arg};

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
