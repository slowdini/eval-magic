//! Write-guard command handlers: the hidden PreToolUse hook entry points and
//! the user-facing `teardown-guard`.
//!
//! The `guard` / `guard-codex` subcommand names are a **stable on-disk
//! contract**: armed hooks staged into harness config reference them by name,
//! so renaming either would break every already-armed guard. Both are aliases
//! of the generic `guard-hook` entry point, which future guard-capable
//! harnesses use directly (`eval-magic guard-hook --harness <name>`).

use std::io;
use std::path::PathBuf;

use crate::adapters::{HarnessAdapter, adapter_for};
use crate::core::Harness;
use crate::sandbox;

/// The hidden Claude Code PreToolUse hook entry point — a frozen alias for
/// `guard-hook --harness claude-code`.
pub(crate) fn run_guard(marker: Option<String>) -> anyhow::Result<()> {
    run_guard_hook("claude-code", marker)
}

/// The hidden Codex PreToolUse hook entry point — a frozen alias for
/// `guard-hook --harness codex`.
pub(crate) fn run_guard_codex(marker: Option<String>) -> anyhow::Result<()> {
    run_guard_hook("codex", marker)
}

/// The generic PreToolUse hook entry point. Reads the hook payload from stdin
/// and the marker path from argv, resolves the harness's guard from the
/// embedded descriptors (hook invocations skip layered discovery — see
/// `cli::run`'s `is_guard_hook` gate), and prints a deny verdict for
/// out-of-bounds calls. It **fails open** — an unknown harness, a guard-less
/// descriptor, or any error path allows the call and exits 0, so the guard can
/// never brick a session.
pub(crate) fn run_guard_hook(harness_name: &str, marker: Option<String>) -> anyhow::Result<()> {
    // Drain stdin before any early return so the harness writing the payload
    // never sees a broken pipe, whatever the verdict.
    let payload = io::read_to_string(io::stdin()).unwrap_or_default();
    let Ok(harness) = Harness::resolve(harness_name) else {
        return Ok(());
    };
    let adapter = adapter_for(harness);
    let Some(marker_path) = marker
        .map(PathBuf::from)
        .or_else(|| default_marker_path(adapter))
    else {
        return Ok(());
    };
    if let Some(verdict) = adapter.guard_verdict(&payload, sandbox::read_marker(&marker_path)) {
        print!("{verdict}");
    }
    Ok(())
}

/// Disarm the write guard for the current directory. Cwd-only by design: the
/// guard lives under harness-local config in the current repo, so this needs no
/// `--skill-dir`/`--skill` flags.
pub(crate) fn run_teardown_guard() -> anyhow::Result<()> {
    let torn = sandbox::teardown_guard(&std::env::current_dir()?);
    println!(
        "{}",
        if torn {
            "🛡 Write guard removed."
        } else {
            "No write guard was installed — nothing to remove."
        }
    );
    Ok(())
}

/// The marker path a guard hook reads when argv carries none: the harness's
/// skills dir under the cwd, e.g. `<cwd>/.claude/skills/.slow-powers-eval-guard.json`.
/// `None` (fail open) for a harness that declares no skills dir.
fn default_marker_path(adapter: &dyn HarnessAdapter) -> Option<PathBuf> {
    Some(
        adapter
            .skills_dir(&std::env::current_dir().unwrap_or_default())?
            .join(sandbox::GUARD_MARKER),
    )
}
