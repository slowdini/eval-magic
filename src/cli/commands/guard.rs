//! Write-guard command handlers: the hidden per-harness PreToolUse hook entry
//! points and the user-facing `teardown-guard`.
//!
//! The `guard` / `guard-codex` subcommand names are a **stable on-disk
//! contract**: armed hooks staged into harness config reference them by name,
//! so renaming either would break every already-armed guard.

use std::io;
use std::path::PathBuf;

use crate::adapters::{adapter_for, claude_code, codex};
use crate::core::Harness;
use crate::sandbox;

/// The hidden Claude Code PreToolUse hook entry point. Reads the hook payload
/// from stdin and the marker path from argv, then prints a deny verdict for
/// out-of-bounds calls. It **fails open** — every error path allows the call
/// and exits 0, so the guard can never brick a session.
pub(crate) fn run_guard(marker: Option<String>) -> anyhow::Result<()> {
    let marker_path = marker
        .map(PathBuf::from)
        .unwrap_or_else(|| default_marker_path("claude-code"));
    let payload = io::read_to_string(io::stdin()).unwrap_or_default();
    if let Some(verdict) =
        claude_code::guard::guard_decision(&payload, sandbox::read_marker(&marker_path))
    {
        print!("{verdict}");
    }
    Ok(())
}

/// The hidden Codex PreToolUse hook entry point. Same policy as `guard`, but
/// Codex blocks by reading `{ "decision": "block", "reason": "..." }` from the
/// hook's stdout.
pub(crate) fn run_guard_codex(marker: Option<String>) -> anyhow::Result<()> {
    let marker_path = marker
        .map(PathBuf::from)
        .unwrap_or_else(|| default_marker_path("codex"));
    let payload = io::read_to_string(io::stdin()).unwrap_or_default();
    if let Some(verdict) =
        codex::guard::guard_decision(&payload, sandbox::read_marker(&marker_path))
    {
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
/// The names are bundled-harness literals because each hook entry point IS its
/// harness's on-disk contract (see the module docs).
fn default_marker_path(harness_name: &str) -> PathBuf {
    let harness = Harness::resolve(harness_name).expect("bundled harness");
    adapter_for(harness)
        .skills_dir(&std::env::current_dir().unwrap_or_default())
        .expect("bundled guard-capable harnesses declare skills_dir")
        .join(sandbox::GUARD_MARKER)
}
