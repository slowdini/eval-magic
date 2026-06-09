//! Small, stateless helpers for the run orchestrator: run-option validation, the
//! per-run nonce, condition naming, plan-mode profile resolution, and display
//! formatting. Extracted from [`super::orchestrate`] so the coordinator stays
//! focused on the build sequence.
//!
//! Ports the `run.ts` helpers `validateHarnessRunOptions`, `nextIteration`,
//! `conditionNamesFor`, `stagingDiscoveryWarning`, and `resolvePlanModeProfile`.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::{Harness, Mode, RunContext};

use super::RunError;
use super::orchestrate::RunOptions;

/// The two condition names for a comparison mode. Ports `conditionNamesFor`.
pub(crate) fn condition_names_for(mode: Mode) -> (&'static str, &'static str) {
    match mode {
        Mode::NewSkill => ("with_skill", "without_skill"),
        Mode::Revision => ("old_skill", "new_skill"),
    }
}

/// The next iteration number for a skill's workspace dir: the explicit override,
/// else one past the highest existing `iteration-<n>`. Ports `nextIteration`.
pub(crate) fn next_iteration(workspace_skill_dir: &Path, override_n: Option<u32>) -> u32 {
    if let Some(n) = override_n {
        return n;
    }
    let Ok(entries) = fs::read_dir(workspace_skill_dir) else {
        return 1;
    };
    let max = entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_prefix("iteration-")
                .and_then(|s| s.parse::<u32>().ok())
        })
        .max();
    max.map_or(1, |m| m + 1)
}

/// Build-time heads-up for the same-session staging limitation on Claude Code
/// (issue #7): `run` stages mid-session, but in-process Task subagents inherit a
/// skill registry fixed at session start, so they never discover the staged
/// skills. Returns the warning, or `None` when it does not apply (staging off, or
/// Codex's fresh-process path). Ports `stagingDiscoveryWarning`.
pub(crate) fn staging_discovery_warning(harness: Harness, no_stage: bool) -> Option<String> {
    if no_stage || harness != Harness::ClaudeCode {
        return None;
    }
    Some(
        [
            "\n⚠ Staged skill discovery requires the staged skills to exist at session start,",
            "  but `run` stages them mid-session. Subagents dispatched from this same session",
            "  (in-process via the Task tool) won't discover them, so every with-skill arm falls",
            "  back. Use one of the two valid paths:",
            "    1. dispatch the subagents from a fresh Claude Code session started after the",
            "       workspace is built, so the staged skills are discovered at session start; or",
            "    2. re-run with --no-stage to inline each condition's SKILL.md into the dispatch",
            "       prompt (correct when the description: frontmatter is unchanged, since there's",
            "       nothing to measure on the discovery axis).",
            "  Either way, run detect-stray-writes (folded into `ingest`) before trusting a staged",
            "  result — it flags live-source reads that reveal a discovery miss after the fact.",
        ]
        .join("\n"),
    )
}

/// Resolve the verbatim plan-mode procedure profile for a harness (issue #142).
/// The profile is a compile-time bundled asset (mirroring the schema embedding in
/// `validation`); a harness without one gets a clear error rather than a silent
/// no-op. Ports `resolvePlanModeProfile`.
pub(crate) fn resolve_plan_mode_profile(harness: Harness) -> Result<&'static str, RunError> {
    match harness {
        Harness::ClaudeCode => Ok(include_str!("../../../profiles/claude-code/plan-mode.md")),
        Harness::Codex => Err(RunError::msg(
            "--plan-mode: no plan-mode profile exists for harness 'codex'. This is a Claude-tier \
             fidelity layer; a harness without a profile leaves the portable dispatch contract \
             unchanged.",
        )),
    }
}

/// Reject the Claude-tier features Codex support does not yet cover. Ports
/// `validateHarnessRunOptions`.
pub(crate) fn validate_harness_run_options(
    opts: &RunOptions,
    ctx: &RunContext,
) -> Result<(), RunError> {
    if ctx.harness != Harness::Codex {
        return Ok(());
    }
    let mut unsupported: Vec<&str> = Vec::new();
    if opts.guard {
        unsupported.push("--guard");
    }
    if ctx.bootstrap_path.is_some() && opts.no_stage {
        unsupported.push("--bootstrap with --no-stage");
    }
    if opts.plan_mode {
        unsupported.push("--plan-mode");
    }
    if opts.stage_name.is_some() && opts.no_stage {
        unsupported.push("--stage-name with --no-stage");
    }
    if unsupported.is_empty() {
        Ok(())
    } else {
        Err(RunError::msg(format!(
            "Codex harness support does not cover every Claude-tier feature yet. Unsupported for \
             Codex: {}.",
            unsupported.join(", ")
        )))
    }
}

/// A per-run nonce (`<millis-base36>-<6 hex>`) that namespaces dispatch
/// descriptions so transcripts can't collide across iterations sharing one parent
/// session's subagents dir. The TS original uses `crypto.randomBytes`; with no
/// RNG crate, the low bits of the sub-millisecond clock supply the entropy —
/// enough, since the base36 millis prefix already differs between runs.
pub(crate) fn make_run_nonce() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}-{:06x}",
        to_base36(now.as_millis() as u64),
        now.subsec_nanos() & 0x00ff_ffff
    )
}

fn to_base36(mut n: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

pub(crate) fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::NewSkill => "new-skill",
        Mode::Revision => "revision",
    }
}

pub(crate) fn harness_label(harness: Harness) -> &'static str {
    match harness {
        Harness::ClaudeCode => "claude-code",
        Harness::Codex => "codex",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warns_for_staged_claude_code_naming_both_paths() {
        let warning = staging_discovery_warning(Harness::ClaudeCode, false).unwrap();
        assert!(warning.contains("fresh"));
        assert!(warning.contains("--no-stage"));
        assert!(warning.contains("detect-stray-writes"));
    }

    #[test]
    fn silent_when_no_stage() {
        assert!(staging_discovery_warning(Harness::ClaudeCode, true).is_none());
    }

    #[test]
    fn silent_for_codex() {
        assert!(staging_discovery_warning(Harness::Codex, false).is_none());
    }

    #[test]
    fn base36_roundtrips_small_values() {
        assert_eq!(to_base36(0), "0");
        assert_eq!(to_base36(35), "z");
        assert_eq!(to_base36(36), "10");
    }

    #[test]
    fn next_iteration_uses_override_then_scans() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(next_iteration(tmp.path(), Some(7)), 7);
        assert_eq!(next_iteration(&tmp.path().join("nope"), None), 1);
        fs::create_dir_all(tmp.path().join("iteration-1")).unwrap();
        fs::create_dir_all(tmp.path().join("iteration-4")).unwrap();
        fs::create_dir_all(tmp.path().join("not-an-iteration")).unwrap();
        assert_eq!(next_iteration(tmp.path(), None), 5);
    }
}
