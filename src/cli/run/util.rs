//! Small, stateless helpers for the run orchestrator: run-option validation, the
//! per-run nonce, condition naming, plan-mode profile resolution, and display
//! formatting. Extracted from [`super::orchestrate`] so the coordinator stays
//! focused on the build sequence.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::adapters::adapter_for;
use crate::adapters::registry::has_embedded_layer;
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

/// Resolve the shared, harness-agnostic plan-mode procedure profile injected by
/// `--plan-mode`. A compile-time bundled asset, mirroring the schema embedding in
/// `validation`.
pub(crate) fn resolve_plan_mode_profile() -> &'static str {
    include_str!("../../../profiles/shared/plan-mode.md")
}

/// The harness preflight verdict: possibly-adjusted run options plus the
/// warnings to print, each naming the fallback that carries the run.
pub(crate) struct HarnessPreflight<'a> {
    pub opts: RunOptions<'a>,
    pub warnings: Vec<String>,
}

/// Check the run options against the selected harness's declared enhancements
/// — the #126 model: a missing enhancement *warns* naming its fallback and the
/// run continues degraded; only genuinely contradictory flag combinations
/// (options the harness declares incompatible with `--no-stage`) and `--guard`
/// on a harness defined by user-supplied descriptors alone (guards are
/// embedded-only) stay errors.
///
/// Adjustments: `--guard` on a guard-less harness is forced off (the
/// detect-stray-writes audit is the fallback), and a harness without a
/// `skills_dir` forces `--no-stage` (each SKILL.md is inlined into its
/// dispatch prompt).
pub(crate) fn harness_run_preflight<'a>(
    opts: &RunOptions<'a>,
    ctx: &RunContext,
) -> Result<HarnessPreflight<'a>, RunError> {
    let adapter = adapter_for(ctx.harness);
    let capabilities = adapter.run_capabilities();
    let label = harness_label(ctx.harness);

    // Contradictory-flag declarations stay hard errors: the harness's staging
    // mechanism conflicts with these options, so no fallback can honor them.
    let mut unsupported: Vec<&str> = Vec::new();
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
    if !unsupported.is_empty() {
        return Err(RunError::msg(format!(
            "Unsupported for --harness {}: {}.",
            label,
            unsupported.join(", ")
        )));
    }

    // `--guard` on a harness defined only by user-supplied descriptors is a
    // hard error, not a downgrade: the write guard stays restricted to
    // built-in descriptors (it fails open, so a mistyped user descriptor
    // would silently disarm it), and a run the user asked to guard must not
    // continue silently unguarded.
    if opts.guard && !capabilities.supports_guard && !has_embedded_layer(ctx.harness) {
        return Err(RunError::msg(format!(
            "--guard: --harness {label} comes from user-supplied descriptors only, and the \
             write guard stays restricted to built-in harnesses (it fails open, so a mistyped \
             descriptor would silently disarm it). Rerun without --guard — out-of-bounds \
             writes are detected after the fact by the detect-stray-writes audit (folded \
             into `ingest`)."
        )));
    }

    let mut opts = opts.clone();
    let mut warnings = Vec::new();
    if opts.guard && !capabilities.supports_guard {
        opts.guard = false;
        warnings.push(format!(
            "--guard: --harness {label} declares no write guard — continuing unguarded; \
             out-of-bounds writes are detected after the fact by the detect-stray-writes \
             audit (folded into `ingest`), never blocked."
        ));
    }
    if !opts.no_stage && adapter.skills_dir(Path::new(".")).is_none() {
        opts.no_stage = true;
        warnings.push(format!(
            "--harness {label} declares no skills_dir — native staging is unavailable; \
             falling back to --no-stage (each SKILL.md is inlined into its dispatch prompt)."
        ));
    }
    if adapter.cli_events_filename().is_none() {
        warnings.push(format!(
            "--harness {label} declares no transcript parser — transcript_check assertions \
             will grade as unverifiable and llm_judge carries the grading; tokens/duration \
             go unrecorded. Recover each final message into outputs/final-message.md \
             (see RUNBOOK.md)."
        ));
    }
    if (opts.agent_model.is_some() || opts.judge_model.is_some())
        && adapter.cli_model_flag().is_none()
    {
        warnings.push(format!(
            "--harness {label} declares no model flag — models are recorded in \
             conditions.json as provenance only; dispatches run on the harness's \
             default model."
        ));
    }
    Ok(HarnessPreflight { opts, warnings })
}

/// A per-run nonce (`<millis-base36>-<6 hex>`) that namespaces dispatch
/// descriptions so they stay unique across iterations of the same skill. With no
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

pub(crate) fn harness_label(harness: Harness) -> String {
    adapter_for(harness).label()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{DetectInput, detect_run_context};
    use std::fs;

    /// Build a `RunContext` for `harness` against a throwaway skill dir.
    fn ctx_for(harness: Harness) -> (tempfile::TempDir, RunContext) {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill = tmp.path().join("widget");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: widget\ndescription: t\n---\n\nbody\n",
        )
        .unwrap();
        let ctx = detect_run_context(DetectInput {
            skill: Some(skill.display().to_string()),
            harness: Some(harness),
            cwd: Some(tmp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();
        (tmp, ctx)
    }

    #[test]
    fn claude_preflight_is_quiet_and_keeps_guard() {
        // `claude -p` loads the project `.claude/settings.local.json` PreToolUse
        // hook from its cwd, so the write guard fires under CLI dispatch. A
        // fully-enhanced harness produces no fallback warnings.
        let (_t, ctx) = ctx_for(Harness::resolve("claude-code").unwrap());
        let opts = RunOptions {
            guard: true,
            ..Default::default()
        };
        let preflight = harness_run_preflight(&opts, &ctx).unwrap();
        assert!(preflight.opts.guard);
        assert!(preflight.warnings.is_empty(), "{:?}", preflight.warnings);
    }

    #[test]
    fn guard_on_a_guardless_harness_warns_and_continues_unguarded() {
        let (_t, ctx) = ctx_for(Harness::resolve("opencode").unwrap());
        let opts = RunOptions {
            guard: true,
            ..Default::default()
        };
        let preflight = harness_run_preflight(&opts, &ctx).unwrap();
        assert!(!preflight.opts.guard, "guard is forced off, not rejected");
        let warning = preflight
            .warnings
            .iter()
            .find(|w| w.contains("--guard"))
            .expect("a guard warning fires");
        assert!(
            warning.contains("detect-stray-writes"),
            "names the fallback: {warning}"
        );
        assert!(warning.contains("never blocked"), "{warning}");
    }

    #[test]
    fn transcriptless_harness_warns_naming_the_llm_judge_fallback() {
        let (_t, ctx) = ctx_for(Harness::resolve("opencode").unwrap());
        let preflight = harness_run_preflight(&RunOptions::default(), &ctx).unwrap();
        let warning = preflight
            .warnings
            .iter()
            .find(|w| w.contains("transcript"))
            .expect("a transcript warning fires");
        assert!(warning.contains("unverifiable"), "{warning}");
        assert!(
            warning.contains("llm_judge"),
            "names the fallback: {warning}"
        );
        assert!(warning.contains("final-message.md"), "{warning}");
    }

    #[test]
    fn model_flags_without_a_descriptor_model_flag_warn_provenance_only() {
        let (_t, ctx) = ctx_for(Harness::resolve("opencode").unwrap());
        let opts = RunOptions {
            agent_model: Some("some-model"),
            ..Default::default()
        };
        let preflight = harness_run_preflight(&opts, &ctx).unwrap();
        let warning = preflight
            .warnings
            .iter()
            .find(|w| w.contains("model flag"))
            .expect("a model warning fires");
        assert!(
            warning.contains("provenance"),
            "names the fallback: {warning}"
        );
    }

    #[test]
    fn no_model_warning_when_no_models_are_requested() {
        let (_t, ctx) = ctx_for(Harness::resolve("opencode").unwrap());
        let preflight = harness_run_preflight(&RunOptions::default(), &ctx).unwrap();
        assert!(
            !preflight.warnings.iter().any(|w| w.contains("model flag")),
            "{:?}",
            preflight.warnings
        );
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
    fn plan_mode_profile_is_shared_and_harness_agnostic() {
        let profile = resolve_plan_mode_profile();
        assert!(profile.contains("Plan mode is active"));
        // Harness-agnostic content: no Claude-specific ExitPlanMode rail or
        // Codex-specific <proposed_plan> block.
        assert!(!profile.contains("ExitPlanMode"));
        assert!(!profile.contains("<proposed_plan>"));
    }

    #[test]
    fn harness_label_opencode() {
        assert_eq!(
            harness_label(Harness::resolve("opencode").unwrap()),
            "opencode"
        );
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
