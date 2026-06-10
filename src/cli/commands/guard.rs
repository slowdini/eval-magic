//! Write-guard command handlers: the hidden `guard` PreToolUse hook entry point
//! and the user-facing `teardown-guard`.

use std::io;
use std::path::PathBuf;

use crate::sandbox;

/// The hidden PreToolUse hook entry point. Reads the hook payload from stdin and
/// the marker path from argv, then prints a deny verdict for out-of-bounds calls.
/// It **fails open** — every error path allows the call and exits 0, so the
/// guard can never brick a session.
pub(crate) fn run_guard(marker: Option<String>) -> anyhow::Result<()> {
    let marker_path = marker
        .map(PathBuf::from)
        .unwrap_or_else(default_marker_path);
    let payload = io::read_to_string(io::stdin()).unwrap_or_default();
    if let Some(verdict) = sandbox::guard_decision(&payload, sandbox::read_marker(&marker_path)) {
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
        .unwrap_or_else(default_codex_marker_path);
    let payload = io::read_to_string(io::stdin()).unwrap_or_default();
    if let Some(verdict) =
        sandbox::codex_guard_decision(&payload, sandbox::read_marker(&marker_path))
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

/// The marker path the guard reads when argv carries none:
/// `<cwd>/.claude/skills/.slow-powers-eval-guard.json`.
fn default_marker_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_default()
        .join(".claude")
        .join("skills")
        .join(sandbox::GUARD_MARKER)
}

/// The marker path the Codex guard reads when argv carries none:
/// `<cwd>/.agents/skills/.slow-powers-eval-guard.json`.
fn default_codex_marker_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_default()
        .join(".agents")
        .join("skills")
        .join(sandbox::GUARD_MARKER)
}
