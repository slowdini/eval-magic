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
use crate::cli::args::CommonArgs;
use crate::cli::{iteration_dir, resolve_iteration, run_context_from, staged_env_roots};
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

/// Disarm the write guard: at the invocation cwd, and — when the shared target
/// flags resolve a run — in every per-`(group, condition)` env of the selected
/// iteration.
///
/// The cwd sweep needs no flags, so disarming from inside a task env still works
/// bare. The env walk is best-effort, because the guard is only *usually*
/// reachable from where the operator stands: a cwd that resolves no skill, or a
/// skill with no such iteration, leaves those guards armed. That case reports
/// what it could not check rather than the all-clear it never established — this
/// command exists for mid-run hand-editing, which is exactly when a false
/// "nothing to remove" costs the most (#298).
///
/// Guard-only by design: the staged skill set and the workspace are what full
/// `teardown` additionally removes.
pub(crate) fn run_teardown_guard(args: CommonArgs) -> anyhow::Result<()> {
    // Cwd first. When the cwd *is* a task env, its guard is already gone by the
    // time the walk below reaches it, so no guard is counted in both scopes.
    let cwd_torn = sandbox::teardown_guard(&std::env::current_dir()?);

    let mut envs_torn = 0usize;
    let mut checked: Option<(u32, usize)> = None;
    let mut unchecked: Option<String> = None;
    match run_context_from(&args).and_then(|ctx| {
        let iteration = resolve_iteration(&ctx, args.iteration)?;
        let dir = iteration_dir(&ctx, Some(iteration))?;
        Ok((iteration, staged_env_roots(&dir)))
    }) {
        Ok((iteration, envs)) => {
            for env in &envs {
                if sandbox::teardown_guard(env) {
                    envs_torn += 1;
                }
            }
            checked = Some((iteration, envs.len()));
        }
        Err(error) => unchecked = Some(error.to_string()),
    }

    // Takes the iteration rather than reading `checked`: naming a count of envs
    // without the iteration they belong to is meaningless, and there is no such
    // thing as a sensible default for it.
    let envs_phrase = |iteration: u32, count: usize| {
        format!(
            "{count} task env{} in iteration {iteration}",
            if count == 1 { "" } else { "s" }
        )
    };
    let mut removed = Vec::new();
    if cwd_torn {
        removed.push("the invocation cwd".to_string());
    }
    if let Some((iteration, _)) = checked
        && envs_torn > 0
    {
        removed.push(envs_phrase(iteration, envs_torn));
    }
    if removed.is_empty() {
        let mut scopes = vec!["the invocation cwd".to_string()];
        if let Some((iteration, count)) = checked {
            scopes.push(envs_phrase(iteration, count));
        }
        println!(
            "No write guard was installed — nothing to remove (checked {}).",
            scopes.join(" and ")
        );
    } else {
        println!("🛡 Write guard removed: {}.", removed.join(", "));
    }
    if let Some(reason) = unchecked {
        // The resolution error already names the flags that would have resolved
        // a run, so this adds only what it cannot know: that guards may survive,
        // and that full `teardown` is the other way to reach them.
        eprintln!(
            "⚠ Task env guards were not checked, so any that were armed still are: \
             {reason}.\n   Add the run's target flags, or run `eval-magic teardown`."
        );
    }
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
