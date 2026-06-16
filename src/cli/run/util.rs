//! Small, stateless helpers for the run orchestrator: run-option validation, the
//! per-run nonce, condition naming, plan-mode profile resolution, and display
//! formatting. Extracted from [`super::orchestrate`] so the coordinator stays
//! focused on the build sequence.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::{Harness, Mode, RunContext};

use super::RunError;
use super::orchestrate::RunOptions;

/// The two condition names for a comparison mode.
pub(crate) fn condition_names_for(mode: Mode) -> (&'static str, &'static str) {
    match mode {
        Mode::NewSkill => ("with_skill", "without_skill"),
        Mode::Revision => ("old_skill", "new_skill"),
    }
}

/// The next iteration number for a skill's workspace dir: the explicit override,
/// else one past the highest existing `iteration-<n>`.
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

/// Build-time heads-up for the same-session staging limitation on Claude Code:
/// `run` stages mid-session, but in-process Task subagents inherit a skill
/// registry fixed at session start, so they never discover the staged skills.
/// Returns the warning, or `None` when it does not apply (staging off, or
/// Codex's fresh-process path).
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

/// The combined "what to do now" upshot when *both* build-time hazards apply at
/// once: staged skills won't be discovered by in-process subagents
/// ([`staging_discovery_warning`]'s condition) AND an installed plugin shadows
/// the control arm. Each warning is clear alone, but together the only valid
/// recovery takes some reasoning — so spell it out. `None` unless both hold.
pub(crate) fn staging_plugin_shadow_action(
    harness: Harness,
    no_stage: bool,
    has_shadows: bool,
) -> Option<String> {
    // Mirror the staging-discovery gate: this only bites staged Claude Code runs.
    let staging_bites = !no_stage && harness == Harness::ClaudeCode;
    if !staging_bites || !has_shadows {
        return None;
    }
    Some(
        [
            "\n▶ Bottom line: both hazards above apply to this run — in-process subagents won't",
            "  discover the staged skill (so with-skill arms fall back to no skill), AND an",
            "  installed plugin shadows the staged copy (so the control arm isn't skill-absent).",
            "  Two clean ways out:",
            "    1. dispatch from a fresh, isolated Claude Code session with the shadowing plugin",
            "       disabled — staging is discovered at session start and the control arm is clean; or",
            "    2. re-run with --no-stage AND disable the shadowing plugin — inlines SKILL.md into",
            "       the prompt and leaves nothing for the plugin to shadow.",
            "  Until then, treat with-skill arms as fallen-back and the control arm as contaminated.",
        ]
        .join("\n"),
    )
}

/// Run-summary heads-up that a `--no-stage` run is unguarded: the write guard
/// requires staging, so `--no-stage` can't arm it, and stray writes are only
/// *detected* after the fact by `detect-stray-writes`. `None` for staged runs.
pub(crate) fn unguarded_notice(no_stage: bool) -> Option<String> {
    if !no_stage {
        return None;
    }
    Some(
        "\nℹ --no-stage run is unguarded — the write guard requires staging, so stray writes are \
         only detected after the fact by detect-stray-writes (folded into `ingest`), never blocked."
            .to_string(),
    )
}

/// Resolve the verbatim plan-mode procedure profile for a harness.
/// The profile is a compile-time bundled asset, mirroring the schema embedding in
/// `validation`.
pub(crate) fn resolve_plan_mode_profile(harness: Harness) -> Result<&'static str, RunError> {
    match harness {
        Harness::ClaudeCode => Ok(include_str!("../../../profiles/claude-code/plan-mode.md")),
        Harness::Codex => Ok(include_str!("../../../profiles/codex/plan-mode.md")),
        Harness::OpenCode => Ok(include_str!("../../../profiles/opencode/plan-mode.md")),
    }
}

/// Reject the Claude-tier features Codex support does not yet cover.
pub(crate) fn validate_harness_run_options(
    opts: &RunOptions,
    ctx: &RunContext,
) -> Result<(), RunError> {
    if ctx.harness != Harness::Codex {
        return Ok(());
    }
    let mut unsupported: Vec<&str> = Vec::new();
    if ctx.bootstrap_path.is_some() && opts.no_stage {
        unsupported.push("--bootstrap with --no-stage");
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
/// session's subagents dir. With no RNG crate, the low bits of the
/// sub-millisecond clock supply the entropy — enough, since the base36 millis
/// prefix already differs between runs.
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
        Harness::OpenCode => "opencode",
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
    fn silent_for_opencode() {
        assert!(staging_discovery_warning(Harness::OpenCode, false).is_none());
    }

    #[test]
    fn combined_action_when_staging_and_shadow_both_apply() {
        let action = staging_plugin_shadow_action(Harness::ClaudeCode, false, true).unwrap();
        // Names both clean ways out: a fresh isolated session, or --no-stage + disable.
        assert!(action.contains("fresh"), "offers a fresh session: {action}");
        assert!(action.contains("--no-stage"), "offers --no-stage: {action}");
        assert!(
            action.to_lowercase().contains("disable"),
            "says to disable the plugin: {action}"
        );
    }

    #[test]
    fn no_combined_action_without_shadow() {
        assert!(staging_plugin_shadow_action(Harness::ClaudeCode, false, false).is_none());
    }

    #[test]
    fn no_combined_action_under_no_stage() {
        assert!(staging_plugin_shadow_action(Harness::ClaudeCode, true, true).is_none());
    }

    #[test]
    fn no_combined_action_for_codex() {
        assert!(staging_plugin_shadow_action(Harness::Codex, false, true).is_none());
    }

    #[test]
    fn unguarded_notice_when_no_stage() {
        let notice = unguarded_notice(true).unwrap();
        assert!(
            notice.to_lowercase().contains("unguarded"),
            "calls the run unguarded: {notice}"
        );
        assert!(
            notice.contains("detect-stray-writes"),
            "names the after-the-fact backstop: {notice}"
        );
    }

    #[test]
    fn no_unguarded_notice_when_staging() {
        assert!(unguarded_notice(false).is_none());
    }

    #[test]
    fn opencode_plan_mode_profile_resolves() {
        let profile = resolve_plan_mode_profile(Harness::OpenCode).unwrap();
        assert!(profile.contains("OpenCode plan mode is active"));
        assert!(profile.contains("plan agent"));
    }

    #[test]
    fn harness_label_opencode() {
        assert_eq!(harness_label(Harness::OpenCode), "opencode");
    }

    #[test]
    fn codex_plan_mode_profile_resolves() {
        let profile = resolve_plan_mode_profile(Harness::Codex).unwrap();
        assert!(profile.contains("Codex plan mode is active"));
        assert!(profile.contains("<proposed_plan>"));
        assert!(!profile.contains("ExitPlanMode"));
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
