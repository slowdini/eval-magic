//! Small, stateless helpers for the run orchestrator: run-option validation, the
//! per-run nonce, condition naming, and display formatting. Extracted from [`super::orchestrate`] so the coordinator stays
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

/// The harness preflight verdict: possibly-adjusted run options plus the
/// warnings to print for optional capabilities using a fallback.
pub(crate) struct HarnessPreflight<'a> {
    pub opts: RunOptions<'a>,
    pub warnings: Vec<String>,
}

/// Check the run options against the selected harness's runner contract and
/// optional enhancements. Dispatch and transcript recovery are mandatory.
/// Other supported enhancements are provided automatically (the write guard
/// auto-arms when declared and staging is active), while optional omissions
/// warn when a lower-fidelity fallback is used. Contradictory flag combinations
/// and an explicit `--guard` on a user-descriptor-only harness remain errors.
///
/// Adjustments: `opts.guard` arrives tri-state (`None` = auto) and leaves
/// resolved to `Some`; a harness without a `skills_dir` forces `--no-stage`
/// (each SKILL.md is inlined into its dispatch prompt).
pub(crate) fn harness_run_preflight<'a>(
    opts: &RunOptions<'a>,
    ctx: &RunContext,
) -> Result<HarnessPreflight<'a>, RunError> {
    let adapter = adapter_for(ctx.harness);
    let capabilities = adapter.run_capabilities();
    let label = harness_label(ctx.harness);

    if !adapter.has_dispatch_recipes() {
        return Err(RunError::msg(format!(
            "--harness {label} declares no dispatch exec template, so it is not runner-ready. \
             Add `[dispatch].exec_template` to the descriptor (see `eval-magic docs byoh`)."
        )));
    }
    if adapter.cli_events_filename().is_none() {
        return Err(RunError::msg(format!(
            "--harness {label} declares no transcript parser, so it is not runner-ready. Add a \
             `[transcript]` parser or extract mapping that recovers the final response (see \
             `eval-magic docs byoh`)."
        )));
    }

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

    // An explicit `--guard` on a harness defined only by user-supplied
    // descriptors is a hard error, not a downgrade: the write guard stays
    // restricted to built-in descriptors (it fails open, so a mistyped user
    // descriptor would silently disarm it), and a run the user asked to guard
    // must not continue silently unguarded. Auto-arm never errors here — it
    // quietly stays off (warning below).
    if opts.guard == Some(true) && !capabilities.supports_guard && !has_embedded_layer(ctx.harness)
    {
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

    // Missing native staging forces --no-stage before the guard resolves, so
    // the guard sees the *effective* staging state.
    if !opts.no_stage && adapter.skills_dir(Path::new(".")).is_none() {
        opts.no_stage = true;
        warnings.push(format!(
            "--harness {label} declares no skills_dir — native staging is unavailable; \
             falling back to --no-stage (each SKILL.md is inlined into its dispatch prompt)."
        ));
    }

    // Resolve the guard tri-state. The guard requires staging and a declared
    // (embedded built-in) guard block; auto mode arms it whenever both hold.
    let can_arm = capabilities.supports_guard && has_embedded_layer(ctx.harness) && !opts.no_stage;
    match opts.guard {
        Some(true) if !capabilities.supports_guard => {
            opts.guard = Some(false);
            warnings.push(format!(
                "--guard: --harness {label} declares no write guard — continuing unguarded; \
                 out-of-bounds writes are detected after the fact by the detect-stray-writes \
                 audit (folded into `ingest`), never blocked."
            ));
        }
        Some(true) if opts.no_stage => {
            opts.guard = Some(false);
            warnings.push(
                "--guard: --no-stage disables the write guard (it requires staging) — \
                 continuing unguarded; out-of-bounds writes are detected after the fact by \
                 the detect-stray-writes audit (folded into `ingest`), never blocked."
                    .to_string(),
            );
        }
        Some(_) => {}
        None => {
            opts.guard = Some(can_arm);
            if !capabilities.supports_guard {
                warnings.push(format!(
                    "--harness {label} declares no write guard — the run continues unguarded; \
                     out-of-bounds writes are detected after the fact by the \
                     detect-stray-writes audit (folded into `ingest`), never blocked. Pass \
                     --no-guard to acknowledge and silence this."
                ));
            }
            // A supported guard on a no-stage run stays off without a warning:
            // the run-summary unguarded notice already covers it.
        }
    }

    if !adapter.surfaces_permission_denials() {
        warnings.push(format!(
            "--harness {label} cannot tell a permission-denied tool result from an ordinary tool \
             error — a dispatch whose calls were refused (and so fell back to static reasoning) \
             will not be flagged in benchmark.json validity_warnings. Read the transcripts before \
             trusting a run whose evals depend on the agent actually executing something."
        ));
    }
    if (opts.agent_model.is_some() || opts.judge_model.is_some() || opts.responder_model.is_some())
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
            guard: Some(true),
            ..Default::default()
        };
        let preflight = harness_run_preflight(&opts, &ctx).unwrap();
        assert_eq!(preflight.opts.guard, Some(true));
        assert!(preflight.warnings.is_empty(), "{:?}", preflight.warnings);
    }

    #[test]
    fn guard_auto_arms_on_a_supported_staged_run() {
        // No guard flag at all: the enhancement is detected and provided
        // automatically (#126), with no warning to acknowledge.
        let (_t, ctx) = ctx_for(Harness::resolve("claude-code").unwrap());
        let preflight = harness_run_preflight(&RunOptions::default(), &ctx).unwrap();
        assert_eq!(preflight.opts.guard, Some(true), "auto-arm resolves to on");
        assert!(preflight.warnings.is_empty(), "{:?}", preflight.warnings);
    }

    #[test]
    fn guard_auto_stays_off_quietly_with_no_stage() {
        // Auto-arm never nags: an unstageable run stays unguarded without a
        // preflight warning (the run-summary unguarded notice covers it).
        let (_t, ctx) = ctx_for(Harness::resolve("claude-code").unwrap());
        let opts = RunOptions {
            no_stage: true,
            ..Default::default()
        };
        let preflight = harness_run_preflight(&opts, &ctx).unwrap();
        assert_eq!(preflight.opts.guard, Some(false));
        assert!(preflight.warnings.is_empty(), "{:?}", preflight.warnings);
    }

    #[test]
    fn guard_auto_arms_on_every_guarded_builtin() {
        // Three built-ins declare a write guard since #155 (Cline does not, so
        // it warns and continues unguarded), so auto mode arms without any
        // guard warning. The guardless-auto warning is only
        // reachable on user-only harnesses now — pinned in tests/run/byoh.rs.
        for name in ["claude-code", "codex", "opencode"] {
            let (_t, ctx) = ctx_for(Harness::resolve(name).unwrap());
            let preflight = harness_run_preflight(&RunOptions::default(), &ctx).unwrap();
            assert_eq!(preflight.opts.guard, Some(true), "{name} auto-arms");
            assert!(
                !preflight.warnings.iter().any(|w| w.contains("write guard")),
                "{name}: no guard warning when armed: {:?}",
                preflight.warnings
            );
        }
    }

    #[test]
    fn no_guard_opts_out_without_warnings() {
        for name in ["claude-code", "opencode"] {
            let (_t, ctx) = ctx_for(Harness::resolve(name).unwrap());
            let opts = RunOptions {
                guard: Some(false),
                ..Default::default()
            };
            let preflight = harness_run_preflight(&opts, &ctx).unwrap();
            assert_eq!(preflight.opts.guard, Some(false));
            assert!(
                !preflight.warnings.iter().any(|w| w.contains("write guard")),
                "--no-guard acknowledges the state, no warning: {:?}",
                preflight.warnings
            );
        }
    }

    #[test]
    fn explicit_guard_with_no_stage_warns_and_continues_unguarded() {
        let (_t, ctx) = ctx_for(Harness::resolve("claude-code").unwrap());
        let opts = RunOptions {
            guard: Some(true),
            no_stage: true,
            ..Default::default()
        };
        let preflight = harness_run_preflight(&opts, &ctx).unwrap();
        assert_eq!(preflight.opts.guard, Some(false));
        let warning = preflight
            .warnings
            .iter()
            .find(|w| w.starts_with("--guard:"))
            .expect("an explicit --guard request that can't be honored warns");
        assert!(warning.contains("--no-stage"), "{warning}");
        assert!(
            warning.contains("detect-stray-writes"),
            "names the fallback: {warning}"
        );
    }

    #[test]
    fn explicit_guard_arms_on_opencode() {
        // An honored explicit --guard warns about nothing. (A --guard request
        // a harness cannot honor is only reachable on user-only harnesses —
        // a hard preflight error pinned in tests/run/byoh.rs.)
        let (_t, ctx) = ctx_for(Harness::resolve("opencode").unwrap());
        let opts = RunOptions {
            guard: Some(true),
            ..Default::default()
        };
        let preflight = harness_run_preflight(&opts, &ctx).unwrap();
        assert_eq!(preflight.opts.guard, Some(true));
        assert!(
            !preflight.warnings.iter().any(|w| w.contains("--guard")),
            "{:?}",
            preflight.warnings
        );
    }

    #[test]
    fn opencode_is_runner_ready() {
        let (_t, ctx) = ctx_for(Harness::resolve("opencode").unwrap());
        let preflight = harness_run_preflight(&RunOptions::default(), &ctx).unwrap();
        assert!(
            !preflight
                .warnings
                .iter()
                .any(|w| w.contains("transcript parser")),
            "runner-ready built-ins do not warn: {:?}",
            preflight.warnings
        );
    }

    #[test]
    fn preflight_does_not_warn_when_denials_can_be_detected() {
        // All three built-in parsers report refusals structurally, so none of
        // them carry the fallback warning. The preflight fallback branch stays
        // defensive for a future harness whose parser cannot tell a refusal
        // from an ordinary tool error.
        for name in ["claude-code", "codex", "opencode"] {
            let (_t, ctx) = ctx_for(Harness::resolve(name).unwrap());
            let preflight = harness_run_preflight(&RunOptions::default(), &ctx).unwrap();
            assert!(
                !preflight
                    .warnings
                    .iter()
                    .any(|w| w.contains("permission-denied")),
                "{name}: {:?}",
                preflight.warnings
            );
        }
    }

    #[test]
    fn wired_built_ins_do_not_warn_about_dispatch_recipes() {
        // The dispatch-recipe warning for a dispatchless harness is pinned on
        // a user descriptor in tests/run/byoh.rs; every built-in wires recipes.
        for name in ["claude-code", "cline", "codex", "opencode"] {
            let (_t, ctx) = ctx_for(Harness::resolve(name).unwrap());
            let preflight = harness_run_preflight(&RunOptions::default(), &ctx).unwrap();
            assert!(
                !preflight
                    .warnings
                    .iter()
                    .any(|w| w.contains("dispatch exec recipe")),
                "{name}: {:?}",
                preflight.warnings
            );
        }
    }

    #[test]
    fn model_flags_with_a_wired_model_flag_do_not_warn() {
        // The provenance-only warning for a harness *without* a model flag is
        // pinned on a user descriptor in tests/run/byoh.rs; every built-in
        // declares one.
        let (_t, ctx) = ctx_for(Harness::resolve("opencode").unwrap());
        let opts = RunOptions {
            agent_model: Some("some-model"),
            ..Default::default()
        };
        let preflight = harness_run_preflight(&opts, &ctx).unwrap();
        assert!(
            !preflight.warnings.iter().any(|w| w.contains("model flag")),
            "{:?}",
            preflight.warnings
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
