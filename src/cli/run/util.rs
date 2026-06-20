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

/// Dispatch instruction for one condition batch: iterate the matching `tasks[]`
/// and dispatch each as a subagent with its `agent_description` verbatim. A building
/// block of the interactive runbook's per-condition steps ([`super::runbook`]).
pub(crate) fn insession_dispatch_batch(condition: &str) -> String {
    format!(
        "iterate the `tasks[]` entries in dispatch.json whose `condition` is `{condition}` and \
         dispatch each as a subagent, passing its `agent_description` verbatim as the subagent \
         description (that string is the key that links each transcript back — without it tool \
         calls, tokens, and duration come back empty)."
    )
}

/// The `switch-condition` barrier command between batches: name the condition about
/// to be dispatched (the one to keep). A building block of the interactive runbook
/// ([`super::runbook`]).
pub(crate) fn insession_switch_command(target_args: &str, iteration: u32, keep: &str) -> String {
    format!("eval-magic switch-condition{target_args} --iteration {iteration} --condition {keep}")
}

/// The `ingest` hand-off command + its session-resolution hint. A building block of
/// the interactive runbook ([`super::runbook`]).
pub(crate) fn insession_ingest_command(target_args: &str, iteration: u32) -> String {
    format!(
        "eval-magic ingest{target_args} --iteration {iteration}\n\
         (ingest auto-resolves the subagents dir from CLAUDE_CODE_SESSION_ID; outside that \
         session, add --session-id <id> or --subagents-dir <path>.)"
    )
}

/// The post-`run` handoff for the isolated in-session flow: cd into the env, start a
/// *fresh* Claude Code session there, and have it read `RUNBOOK.md` — which carries the
/// full dispatch → switch-condition → ingest → finalize loop. The env (incl.
/// `env/.claude/skills/`) is built before that session starts, so the fresh session is
/// structural, not a watcher workaround; the orchestrator no longer juggles the dispatch
/// loop itself.
pub(crate) fn insession_isolated_handoff(env_dir: &Path) -> String {
    format!(
        "start the isolated run in a fresh session:\n  \
         1. cd {env}\n  \
         2. start a fresh Claude Code session there (`claude`)\n  \
         3. say: Read and follow RUNBOOK.md\n\
         RUNBOOK.md walks the whole loop (dispatch → switch-condition → ingest → finalize) and \
         writes benchmark.json; resume here to read it.",
        env = env_dir.display()
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
    fn isolated_handoff_points_into_env_and_at_the_runbook() {
        let env = Path::new("/work/skills-workspace/widget/iteration-3/env");
        let handoff = insession_isolated_handoff(env);
        assert!(
            handoff.contains("/work/skills-workspace/widget/iteration-3/env"),
            "names the env to cd into: {handoff}"
        );
        assert!(handoff.contains("cd "), "spells out the cd step: {handoff}");
        assert!(
            handoff.contains("Read and follow RUNBOOK.md"),
            "hands off to the runbook in a fresh session: {handoff}"
        );
        assert!(
            handoff.contains("fresh"),
            "names the fresh isolated session: {handoff}"
        );
        // The handoff replaces the old printed dispatch loop — it must not re-print it.
        assert!(
            !handoff.contains("one batch at a time"),
            "the dispatch loop lives in RUNBOOK.md now, not the summary: {handoff}"
        );
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
