//! Small, stateless helpers for the run orchestrator: run-option validation, the
//! per-run nonce, condition naming, plan-mode profile resolution, and display
//! formatting. Extracted from [`super::orchestrate`] so the coordinator stays
//! focused on the build sequence.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::adapters::adapter_for;
use crate::core::{Harness, Mode, RunContext, capabilities_for};

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

/// Build-time heads-up about staged-skill discovery on Claude Code, keyed on whether the project
/// `.claude/skills/` dir existed when the orchestrator session started.
///
/// Claude Code's file watcher only watches skill directories that existed at session start. When
/// `.claude/skills/` already existed, live change detection surfaces mid-session-staged skills
/// in-session (and to subagents dispatched afterward) — no fallback, so this returns a short
/// confirmation note. When `run` had to *create* `.claude/skills/`, that new top-level dir isn't
/// watched until the session re-scans (a restart, or a plugin reload / other refresh event), so
/// subagents won't discover the staged skills yet and with-skill arms fall back — this returns the
/// actionable warning. `None` when staging is off or the harness isn't Claude Code (Codex/OpenCode
/// dispatch as fresh processes that rediscover skills each time).
pub(crate) fn staging_discovery_warning(
    harness: Harness,
    no_stage: bool,
    skills_dir_preexisted: bool,
) -> Option<String> {
    if no_stage || harness != Harness::ClaudeCode {
        return None;
    }
    if skills_dir_preexisted {
        return Some(
            [
                "\nℹ Staged into the existing .claude/skills/ — Claude Code's live change detection",
                "  surfaces these skills in-session, so subagents dispatched from this session",
                "  discover them (a freshly-staged skill can lag the watcher by a moment; if you",
                "  created .claude/skills/ after this session started, restart once so it's watched).",
                "  Run detect-stray-writes (folded into `ingest`) to confirm no with-skill arm fell back.",
            ]
            .join("\n"),
        );
    }
    Some(
        [
            "\n⚠ This run created .claude/skills/, which did not exist when your session started.",
            "  Claude Code only watches skill directories that existed at session start, so subagents",
            "  dispatched from this session won't discover the staged skills until the session",
            "  re-scans — with-skill arms fall back until then. The staged skills are now on disk and",
            "  persist, so do one of:",
            "    1. restart this Claude Code session, then dispatch (the staged skills are discovered",
            "       at session start); or",
            "    2. dispatch the subagents from a fresh Claude Code session started after this run; or",
            "    3. re-run with --no-stage to inline each condition's SKILL.md into the dispatch",
            "       prompt (correct when the description: frontmatter is unchanged, since there's",
            "       nothing to measure on the discovery axis).",
            "  Either way, run detect-stray-writes (folded into `ingest`) before trusting a staged",
            "  result — it flags live-source reads that reveal a discovery miss after the fact.",
        ]
        .join("\n"),
    )
}

/// The combined "what to do now" upshot when *both* build-time hazards apply at once: the staged
/// skill won't be discovered by subagents ([`staging_discovery_warning`]'s fresh-dir condition,
/// i.e. `!skills_dir_preexisted`) AND an installed plugin shadows the control arm. Each warning is
/// clear alone, but together the only valid recovery takes some reasoning — so spell it out.
/// `None` unless both hold; when the skills dir pre-existed the staged skill *is* discoverable, so
/// the discovery hazard does not apply and the plain plugin-shadow banner suffices.
pub(crate) fn staging_plugin_shadow_action(
    harness: Harness,
    no_stage: bool,
    has_shadows: bool,
    skills_dir_preexisted: bool,
) -> Option<String> {
    // Mirror the staging-discovery gate: the discovery hazard only bites a staged Claude Code run
    // that had to create .claude/skills/ fresh (otherwise live change detection finds the skill).
    let staging_bites = !no_stage && harness == Harness::ClaudeCode && !skills_dir_preexisted;
    if !staging_bites || !has_shadows {
        return None;
    }
    Some(
        [
            "\n▶ Bottom line: both hazards above apply to this run — this run created",
            "  .claude/skills/ fresh so subagents won't discover the staged skill until the session",
            "  re-scans (with-skill arms fall back to no skill), AND an installed plugin shadows the",
            "  staged copy (so the control arm isn't skill-absent). Two clean ways out:",
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

/// The in-session dispatch-loop guidance shared by the post-`run` "Next:" message
/// ([`super::orchestrate`]) and the interactive runbook
/// ([`super::runbook`]): iterate `tasks[]`, dispatch each as a subagent passing
/// `agent_description` verbatim, then `ingest`. Threads the target selector +
/// iteration into the ingest command so it is copy-pasteable. Keeping it in one
/// place means the printed guidance and the runbook can never drift.
pub(crate) fn insession_dispatch_next_steps(target_args: &str, iteration: u32) -> String {
    format!(
        "iterate the tasks[] array in dispatch.json and dispatch each task as a subagent, \
         passing its `agent_description` verbatim as the subagent description (that string is \
         the key that links each transcript back — without it tool calls, tokens, and duration \
         come back empty). Then run:\n  eval-magic ingest{target_args} --iteration {iteration}\n\
         (ingest auto-resolves the subagents dir from CLAUDE_CODE_SESSION_ID; outside that \
         session, add --session-id <id> or --subagents-dir <path>.)"
    )
}

/// Resolve the verbatim plan-mode procedure profile for a harness.
/// The profile is a compile-time bundled asset, mirroring the schema embedding in
/// `validation`.
pub(crate) fn resolve_plan_mode_profile(harness: Harness) -> Result<&'static str, RunError> {
    Ok(adapter_for(harness).plan_mode_profile())
}

/// Reject run options not supported by the selected harness's current mechanism.
pub(crate) fn validate_harness_run_options(
    opts: &RunOptions,
    ctx: &RunContext,
) -> Result<(), RunError> {
    let capabilities = capabilities_for(ctx.harness);
    let mut unsupported: Vec<&str> = Vec::new();
    if opts.guard && !capabilities.supports_guard {
        unsupported.push("--guard");
    }
    if ctx.bootstrap_path.is_some()
        && opts.no_stage
        && !capabilities.supports_bootstrap_with_no_stage
    {
        unsupported.push("--bootstrap with --no-stage");
    }
    if opts.stage_name.is_some() && opts.no_stage && !capabilities.supports_stage_name_with_no_stage
    {
        unsupported.push("--stage-name with --no-stage");
    }
    if unsupported.is_empty() {
        Ok(())
    } else {
        Err(RunError::msg(format!(
            "Unsupported for --harness {}: {}.",
            harness_label(ctx.harness),
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
    adapter_for(harness).label()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_warning_when_skills_dir_created_fresh() {
        // The skills dir did not exist at session start, so `run` creates it; Claude Code's file
        // watcher won't pick up the new top-level dir until the session re-scans.
        let warning = staging_discovery_warning(Harness::ClaudeCode, false, false).unwrap();
        assert!(
            warning.contains("session start"),
            "names the real cause (watcher only sees dirs present at session start): {warning}"
        );
        assert!(warning.contains("restart"), "offers a restart: {warning}");
        assert!(
            warning.contains("--no-stage"),
            "offers --no-stage: {warning}"
        );
        assert!(
            warning.contains("detect-stray-writes"),
            "names the after-the-fact backstop: {warning}"
        );
        assert!(
            !warning.contains("every with-skill arm falls"),
            "drops the false absolute claim: {warning}"
        );
    }

    #[test]
    fn discovery_note_when_skills_dir_preexisting() {
        // The skills dir already existed at session start, so live change detection surfaces the
        // staged skills in-session — no fallback, just a confirmation + the backstop reminder.
        let note = staging_discovery_warning(Harness::ClaudeCode, false, true).unwrap();
        assert!(
            note.contains("live change detection"),
            "explains why discovery works: {note}"
        );
        assert!(
            note.contains("detect-stray-writes"),
            "still points at the backstop: {note}"
        );
        assert!(
            !note.contains("falls back"),
            "no fallback claim when the skills are discoverable: {note}"
        );
    }

    #[test]
    fn silent_when_no_stage() {
        assert!(staging_discovery_warning(Harness::ClaudeCode, true, false).is_none());
        assert!(staging_discovery_warning(Harness::ClaudeCode, true, true).is_none());
    }

    #[test]
    fn silent_for_codex() {
        assert!(staging_discovery_warning(Harness::Codex, false, false).is_none());
    }

    #[test]
    fn silent_for_opencode() {
        assert!(staging_discovery_warning(Harness::OpenCode, false, false).is_none());
    }

    #[test]
    fn combined_action_when_fresh_dir_and_shadow_both_apply() {
        // The discovery hazard is real only when the dir was created fresh
        // (skills_dir_preexisted = false); paired with a plugin shadow, the recovery takes
        // reasoning — so spell it out.
        let action = staging_plugin_shadow_action(Harness::ClaudeCode, false, true, false).unwrap();
        assert!(
            action.contains("fresh") || action.contains("restart"),
            "offers a clean session: {action}"
        );
        assert!(action.contains("--no-stage"), "offers --no-stage: {action}");
        assert!(
            action.to_lowercase().contains("disable"),
            "says to disable the plugin: {action}"
        );
    }

    #[test]
    fn no_combined_action_when_skills_dir_preexisting() {
        // Dir existed at session start: the staged skill is discoverable, so the discovery hazard
        // does not apply and the plain plugin-shadow banner suffices.
        assert!(staging_plugin_shadow_action(Harness::ClaudeCode, false, true, true).is_none());
    }

    #[test]
    fn no_combined_action_without_shadow() {
        assert!(staging_plugin_shadow_action(Harness::ClaudeCode, false, false, false).is_none());
    }

    #[test]
    fn no_combined_action_under_no_stage() {
        assert!(staging_plugin_shadow_action(Harness::ClaudeCode, true, true, false).is_none());
    }

    #[test]
    fn no_combined_action_for_codex() {
        assert!(staging_plugin_shadow_action(Harness::Codex, false, true, false).is_none());
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
