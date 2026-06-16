//! Phases 3 & 4 — build every `(eval, condition)` dispatch task and write
//! `conditions.json` / `dispatch-manifest.md` / per-task prompts / `dispatch.json`
//! ([`write_dispatch`]), then arm the opt-in write guard and run the plugin-shadow
//! preflight ([`post_build`]).

use std::fs;
use std::path::Path;

use serde_json::json;

use crate::adapters::{detect_plugin_shadows, format_shadow_banner, resolve_config_dir};
use crate::core::{AvailableSkill, ConditionEntry, ConditionsRecord, Harness, RunContext};
use crate::pipeline::io::now_iso8601;
use crate::sandbox::install_guard_for_harness;

use super::super::dispatch::{
    DispatchTaskOpts, build_dispatch_task, build_manifest, copy_fixtures, get_skill_description,
};
use super::super::staging::skills_dir_for_harness;
use super::super::util::{staging_plugin_shadow_action, unguarded_notice};
use super::super::{RunError, write_json};
use super::{Resolved, RunOptions, Staged};

/// Build every `(eval, condition)` dispatch task and write `conditions.json`,
/// `dispatch-manifest.md`, the per-task prompt files, and `dispatch.json`.
/// Returns the number of dispatch tasks.
pub(super) fn write_dispatch(
    ctx: &RunContext,
    opts: &RunOptions,
    r: &Resolved,
    staged: &Staged,
) -> Result<usize, RunError> {
    let conditions = ConditionsRecord {
        mode: r.mode,
        baseline: r.baseline.clone(),
        conditions: vec![
            ConditionEntry {
                name: r.cond_a.to_string(),
                skill_path: r.skill_path_a.clone(),
                staged_skill_slug: Some(staged.cond_a_slug.clone()),
            },
            ConditionEntry {
                name: r.cond_b.to_string(),
                skill_path: r.skill_path_b.clone(),
                staged_skill_slug: Some(staged.cond_b_slug.clone()),
            },
        ],
        timestamp: now_iso8601(),
        harness: Some(ctx.harness),
        run_nonce: Some(r.run_nonce.clone()),
        runs: Some(opts.runs),
        agent_model: opts.agent_model.map(str::to_owned),
        judge_model: opts.judge_model.map(str::to_owned),
        label: opts.label.map(str::to_owned),
    };
    write_json(&r.iteration_dir.join("conditions.json"), &conditions)?;

    let staged_skill_path_for = |cond_slug: Option<&str>| -> Option<String> {
        cond_slug.map(|slug| {
            skills_dir_for_harness(&ctx.stage_root, ctx.harness)
                .join(slug)
                .join("SKILL.md")
                .to_string_lossy()
                .into_owned()
        })
    };

    // availableSkills for a condition = siblings + the skill-under-test when that
    // condition loads it. Empty when nothing was staged.
    let available_skills_for =
        |cond_skill_path: Option<&str>, cond_slug: Option<&str>| -> Vec<AvailableSkill> {
            if opts.no_stage {
                return Vec::new();
            }
            let mut skills = staged.sibling_skills.clone();
            if let Some(csp) = cond_skill_path {
                let name = match (ctx.harness, cond_slug) {
                    (Harness::Codex, Some(slug)) => slug.to_string(),
                    _ => ctx.skill_name.clone(),
                };
                skills.push(AvailableSkill {
                    name,
                    path: staged_skill_path_for(cond_slug).unwrap_or_else(|| csp.to_string()),
                    description: get_skill_description(Path::new(csp)),
                });
            }
            skills
        };

    let mut tasks = Vec::new();
    for ev in &r.selected_evals {
        let eval_dir = r.iteration_dir.join(format!("eval-{}", ev.id));
        fs::create_dir_all(&eval_dir)?;

        for (cond_name, cond_skill_path, cond_slug) in [
            (
                r.cond_a,
                r.skill_path_a.as_deref(),
                staged.cond_a_slug.as_deref(),
            ),
            (
                r.cond_b,
                r.skill_path_b.as_deref(),
                staged.cond_b_slug.as_deref(),
            ),
        ] {
            let cond_dir = eval_dir.join(cond_name);
            let runs = ev.runs.unwrap_or(opts.runs);
            let staged_path = staged_skill_path_for(cond_slug);

            for run_idx in 1..=runs {
                // A single-run cell keeps the flat legacy layout; multi-run
                // cells nest each run under run-<k>/.
                let (run_dir, run_index) = if runs == 1 {
                    (cond_dir.clone(), None)
                } else {
                    (cond_dir.join(format!("run-{run_idx}")), Some(run_idx))
                };
                let outputs_dir = run_dir.join("outputs");
                fs::create_dir_all(&outputs_dir)?;

                let fixtures = copy_fixtures(ev, &ctx.skill_subdir, &run_dir)?;
                let available_skills = available_skills_for(cond_skill_path, cond_slug);
                let outputs_dir_str = outputs_dir.to_string_lossy().into_owned();
                let run_dir_str = run_dir.to_string_lossy().into_owned();

                tasks.push(build_dispatch_task(&DispatchTaskOpts {
                    eval_id: &ev.id,
                    condition: cond_name,
                    skill_path: cond_skill_path,
                    staged_skill_slug: cond_slug,
                    staged_skill_path: staged_path.as_deref(),
                    user_prompt: &ev.prompt,
                    fixtures,
                    outputs_dir: &outputs_dir_str,
                    cond_dir: &run_dir_str,
                    bootstrap_content: staged.bootstrap_content.as_deref(),
                    plan_mode_content: staged.plan_mode_content.as_deref(),
                    skill_name: &ctx.skill_name,
                    available_skills,
                    harness: ctx.harness,
                    run_tag: Some(&r.run_tag),
                    run_index,
                })?);
            }
        }
    }

    let manifest_path = r.iteration_dir.join("dispatch-manifest.md");
    fs::write(
        &manifest_path,
        build_manifest(
            &ctx.skill_name,
            r.mode,
            r.baseline.as_deref(),
            r.iteration,
            &now_iso8601(),
            &tasks,
        ),
    )?;

    // Write each prompt to its own file; dispatch.json references it by path.
    for task in &tasks {
        fs::write(&task.dispatch_prompt_path, &task.dispatch_prompt)?;
    }

    let dispatch_json_path = r.iteration_dir.join("dispatch.json");
    let dispatch_json = json!({
        "skill_name": ctx.skill_name,
        "iteration": r.iteration,
        "run_nonce": r.run_nonce,
        "iteration_dir": r.iteration_dir.to_string_lossy(),
        "mode": r.mode,
        "baseline": r.baseline,
        "plan_mode": opts.plan_mode,
        "runs": opts.runs,
        "agent_model": conditions.agent_model,
        "judge_model": conditions.judge_model,
        "label": conditions.label,
        "conditions": conditions.conditions,
        "harness": ctx.harness,
        "tasks": tasks,
    });
    write_json(&dispatch_json_path, &dispatch_json)?;

    Ok(tasks.len())
}

/// Post-build side effects: arm the opt-in write guard and run the Claude Code
/// plugin-shadow preflight.
pub(super) fn post_build(
    ctx: &RunContext,
    opts: &RunOptions,
    r: &Resolved,
) -> Result<(), RunError> {
    // Opt-in hard guard: a PreToolUse hook blocking subagent writes/installs
    // outside the eval sandbox while dispatches run.
    if opts.guard && !opts.dry_run {
        if opts.no_stage {
            eprintln!("\n⚠ --guard requires staging enabled; skipping guard install.");
        } else {
            install_guard_for_harness(
                &ctx.stage_root,
                &ctx.workspace_root,
                &std::env::current_exe()?,
                ctx.harness,
                None,
            )?;
            match ctx.harness {
                Harness::ClaudeCode => println!(
                    "\n🛡 Write guard armed: a PreToolUse hook is staged in .claude/settings.local.json\n   and will block writes/installs outside the eval sandbox during dispatches.\n   It auto-expires in 6h and is removed on the next run; to remove it now:\n     eval-magic teardown-guard"
                ),
                Harness::Codex => println!(
                    "\n🛡 Write guard armed: a PreToolUse hook is staged in .codex/hooks.json\n   and will block writes/installs outside the eval sandbox during Codex dispatches.\n   Dispatch with codex exec --dangerously-bypass-hook-trust so the vetted eval hook runs.\n   It auto-expires in 6h and is removed on the next run; to remove it now:\n     eval-magic teardown-guard"
                ),
                Harness::OpenCode => unreachable!(
                    "install_guard_for_harness rejects OpenCode before this message prints"
                ),
            }
        }
    }

    // No-stage runs can't arm the guard at all — say so in the summary, whether
    // or not --guard was passed, so the operator knows the run is unguarded.
    if !opts.dry_run
        && let Some(notice) = unguarded_notice(opts.no_stage)
    {
        eprintln!("{notice}");
    }

    // Plugin-shadow preflight (Claude Code): a staged skill name also discoverable
    // from an enabled plugin or the global skills dir contaminates the run.
    if ctx.harness == Harness::ClaudeCode {
        let mut names: Vec<&str> = vec![ctx.skill_name.as_str()];
        names.extend(ctx.sibling_skill_names.iter().map(String::as_str));
        let report = detect_plugin_shadows(&resolve_config_dir(None), &ctx.stage_root, &names);
        if !report.shadowed.is_empty() {
            write_json(&r.iteration_dir.join("plugin-shadow.json"), &report)?;
            eprintln!("{}", format_shadow_banner(&report));
        }
        // When the staging-discovery miss and a plugin shadow both bite, the
        // individual warnings don't add up to an obvious action — summarize it.
        if let Some(action) =
            staging_plugin_shadow_action(ctx.harness, opts.no_stage, !report.shadowed.is_empty())
        {
            eprintln!("{action}");
        }
    }
    Ok(())
}
