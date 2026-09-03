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
//!
//! The resulting two-way reference with `crate::adapters` is intentional.
//! `adapters` owns the harness-facing contract and renders each descriptor's
//! native hook surface and verdict shape; this module owns the harness-neutral
//! enforcement and lifecycle. It consults the adapter registry only where
//! policy or cleanup must account for every harness. Put new code here when it
//! enforces the shared boundary; put harness integration and descriptor
//! rendering in `adapters`.

pub(crate) mod command_policy;
pub mod decide;
mod git_command;
pub mod guard;
pub(crate) mod guard_profiles;
pub mod install;
mod mutation_targets;
pub mod policy;
mod shell_targets;

pub(crate) use decide::marker_is_armed;
pub use decide::{GUARD_REASON_PREFIX, GuardDecision, GuardMarker, decide};
pub(crate) use guard::GuardDenialRecord;
pub(crate) use guard::parse_tool_call;
pub use guard::read_marker;
pub(crate) use install::{GUARD_DENIALS_DIR, GUARD_DENIALS_LOG, guard_is_armed};
pub use install::{GUARD_MANIFEST, GUARD_MARKER, teardown_guard};
pub(crate) use policy::lexically_absolute;
pub use policy::{
    classify_bash, is_patch_tool, is_shell_tool, is_under, is_under_any, is_write_tool, path_arg,
};
pub(crate) use shell_targets::command_reads_literal_path;

use std::time::{SystemTime, UNIX_EPOCH};

/// Conventional task-local directory named in dispatch prompts and actionable
/// guard denials for temporary and scratch work.
pub(crate) const TASK_SCRATCH_DIR: &str = "tmp";

/// The paths the framework itself owns inside every task environment, as
/// gitignore-style patterns anchored at the env root.
///
/// One definition, because the same set has to hold on three surfaces that
/// would otherwise drift: the env's `.git/info/exclude` (so a measurement never
/// reports these as the agent's change), each harness's
/// `framework_ignore_paths` (so the codebase's own linters never report them as
/// project failures), and what `eval-magic docs codebase` promises about both.
pub(crate) fn framework_owned_entries() -> [String; 2] {
    [
        format!("/{GUARD_DENIALS_DIR}/"),
        format!("/{TASK_SCRATCH_DIR}/"),
    ]
}

/// Current wall clock in epoch milliseconds. chrono ships without its `clock`
/// feature (it parses timestamps but never reads the clock), so the time comes
/// from `std::time`. Shared by the guard's expiry check and marker stamping.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
