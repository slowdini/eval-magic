//! Phase 2 — create the iteration dir, (re)stage the condition skills + their
//! siblings, and resolve the shared dispatch-prompt inputs.

use std::fs;
use std::path::Path;

use crate::core::RunContext;
use crate::sandbox::teardown_guard;

use super::super::RunError;
use super::super::dispatch::get_skill_description;
use super::super::fixtures::{FixtureClaims, copy_fixtures};
use super::super::staging::{
    StageSiblingOpts, StageSkillOpts, cleanup_staged_skills, register_staged_skill_for_cleanup,
    skills_dir_for_harness, stage_sibling_skills, stage_skill_for_harness,
};
use super::super::util::{harness_label, resolve_plan_mode_profile};
use super::envs::{EnvLayoutInput, env_targets};
use super::{Resolved, RunOptions, Staged};

pub(super) fn stage_conditions(
    ctx: &RunContext,
    opts: &RunOptions,
    r: &Resolved,
) -> Result<Staged, RunError> {
    fs::create_dir_all(&r.iteration_dir)?;
    fs::copy(&r.skill_md_path, r.iteration_dir.join("skill-snapshot.md"))?;

    let bootstrap_content = match &ctx.bootstrap_path {
        Some(path) => Some(fs::read_to_string(path)?),
        None => None,
    };

    let plan_mode_content = if opts.plan_mode {
        let profile = resolve_plan_mode_profile();
        println!(
            "  plan-mode: injecting the shared plan-mode profile as operating context for {} (issue #142; necessary-not-sufficient fidelity layer)",
            harness_label(ctx.harness)
        );
        Some(profile.to_string())
    } else {
        None
    };

    // Sibling skill `(name, description)`, env-independent. `build` resolves each
    // path per env. Empty when --no-stage.
    let sibling_meta: Vec<(String, String)> = if opts.no_stage {
        Vec::new()
    } else {
        ctx.sibling_skill_names
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    get_skill_description(&ctx.skill_dir.join(name).join("SKILL.md")),
                )
            })
            .collect()
    };

    // --stage-name overrides the conspicuous slug with a verbatim name; it targets
    // the single staging condition, so reject the both-stage case up front.
    if let Some(_stage_name) = opts.stage_name
        && !opts.no_stage
        && r.skill_path_a.is_some()
        && r.skill_path_b.is_some()
    {
        return Err(RunError::msg(
            "--stage-name is only supported when exactly one condition stages the skill (e.g. --mode new-skill); both conditions stage here.",
        ));
    }

    // The environments to stage: one shared `env/` for in-session (hosting both
    // conditions + the first group's fixtures), or one per (group, condition) for
    // Cli (each with only its condition's skill + its group's fixtures).
    let targets = env_targets(&EnvLayoutInput {
        iteration_dir: &r.iteration_dir,
        mechanism: ctx.run_mode.mechanism(),
        groups: &r.groups,
        cond_a: r.cond_a,
        cond_b: r.cond_b,
        skill_path_a: r.skill_path_a.as_deref(),
        skill_path_b: r.skill_path_b.as_deref(),
    });

    let mut cond_a_slug = None;
    let mut cond_b_slug = None;

    for target in &targets {
        // Disarm a prior run's guard before re-staging, so a crashed run can't leave
        // the write-blocking hook armed across runs. Created unconditionally — even
        // under --no-stage, fixtures (and the in-session RUNBOOK) still land here.
        teardown_guard(&target.root);
        fs::create_dir_all(&target.root)?;

        if !opts.no_stage {
            cleanup_staged_skills(&target.root, ctx.harness)?;
            if ctx.stage_siblings {
                stage_sibling_skills(&StageSiblingOpts {
                    skill_under_test: &ctx.skill_name,
                    skills_source_dir: &ctx.skill_dir,
                    repo_root: &target.root,
                    harness: ctx.harness,
                })?;
            }
        }

        for (cond_name, cond_skill_path) in &target.conditions {
            // Refuse to clobber a pre-existing --stage-name dir in this env.
            if let Some(stage_name) = opts.stage_name
                && !opts.no_stage
                && cond_skill_path.is_some()
            {
                let dir = skills_dir_for_harness(&target.root, ctx.harness).join(stage_name);
                if dir.exists() {
                    return Err(RunError::msg(format!(
                        "--stage-name \"{stage_name}\": {} already exists; refusing to clobber it. Remove it or choose a different name.",
                        dir.display()
                    )));
                }
            }

            if let Some(slug) = stage_for(
                ctx,
                opts,
                r,
                cond_name,
                cond_skill_path.as_deref(),
                &target.root,
            )? {
                if *cond_name == r.cond_a {
                    cond_a_slug = Some(slug.clone());
                }
                if *cond_name == r.cond_b {
                    cond_b_slug = Some(slug.clone());
                }
                // A custom-named dir isn't caught by the prefix scan; record it in
                // this env's manifest so cleanup removes it.
                if opts.stage_name == Some(slug.as_str()) {
                    register_staged_skill_for_cleanup(&target.root, &slug, ctx.harness)?;
                }
            }
        }

        // Copy this env's group's fixtures. Claims are per env (each env is
        // independent); grouping has already routed clobbering evals into separate
        // groups, so within one env the same-source/idempotent rule never trips.
        let mut claims = FixtureClaims::new();
        for eval_id in &target.eval_ids {
            if let Some(ev) = r.selected_evals.iter().find(|e| &e.id == eval_id) {
                copy_fixtures(ev, &ctx.skill_subdir, &target.root, &mut claims)?;
            }
        }
    }

    Ok(Staged {
        cond_a_slug,
        cond_b_slug,
        sibling_meta,
        bootstrap_content,
        plan_mode_content,
    })
}

/// Stage one condition's skill into `root` and return its slug; `Ok(None)` when
/// the condition stages no skill (the new-skill control arm) or under --no-stage.
fn stage_for(
    ctx: &RunContext,
    opts: &RunOptions,
    r: &Resolved,
    cond_name: &str,
    cond_skill_path: Option<&str>,
    root: &Path,
) -> Result<Option<String>, RunError> {
    let Some(path) = cond_skill_path.filter(|_| !opts.no_stage) else {
        return Ok(None);
    };
    let content = fs::read_to_string(path)?;
    let slug = stage_skill_for_harness(&StageSkillOpts {
        content: &content,
        iteration: r.iteration,
        condition: cond_name,
        skill_name: &ctx.skill_name,
        repo_root: root,
        assets_dir: Path::new(path).parent(),
        stage_name_override: opts.stage_name,
        harness: ctx.harness,
    })?;
    Ok(Some(slug))
}
