//! Phase 2 — create the iteration dir, (re)stage the condition skills + their
//! siblings, and resolve the shared dispatch-prompt inputs.

use std::fs;
use std::path::Path;

use crate::core::{AvailableSkill, RunContext};
use crate::sandbox::teardown_guard;

use super::super::RunError;
use super::super::dispatch::get_skill_description;
use super::super::staging::{
    StageSiblingOpts, StageSkillOpts, cleanup_staged_skills, register_staged_skill_for_cleanup,
    skills_dir_for_harness, stage_sibling_skills, stage_skill_for_harness,
};
use super::super::util::{harness_label, resolve_plan_mode_profile, staging_discovery_warning};
use super::{Resolved, RunOptions, Staged};

pub(super) fn stage_conditions(
    ctx: &RunContext,
    opts: &RunOptions,
    r: &Resolved,
) -> Result<Staged, RunError> {
    fs::create_dir_all(&r.iteration_dir)?;
    // The isolated env dir: the agent-under-test's cwd and the staging root
    // (`ctx.stage_root` = `iteration_dir/env`). Created unconditionally — even under
    // `--no-stage`, fixtures + RUNBOOK still land here. `create_dir_all` is recursive,
    // so this also guarantees `iteration_dir`. The harness skills dir
    // (`env/.claude/skills`) is created by the staging primitives below once
    // `stage_root` points here.
    fs::create_dir_all(&ctx.stage_root)?;
    fs::copy(&r.skill_md_path, r.iteration_dir.join("skill-snapshot.md"))?;

    // Capture whether the harness skills dir already existed BEFORE this run touches anything:
    // cleanup may prune an empty dir and sibling/skill staging below create it, so reading
    // `.exists()` later would always be true. Claude Code only watches skill dirs that existed at
    // session start, so this is the signal for whether the staged skills are discoverable
    // in-session. See `staging_discovery_warning`.
    let skills_dir_preexisted = skills_dir_for_harness(&ctx.stage_root, ctx.harness).exists();

    // Always disarm a prior run's guard before re-staging, so a crashed run can't
    // leave the write-blocking hook armed across runs.
    teardown_guard(&ctx.stage_root);

    if !opts.no_stage {
        cleanup_staged_skills(&ctx.stage_root, ctx.harness)?;
        if ctx.stage_siblings {
            stage_sibling_skills(&StageSiblingOpts {
                skill_under_test: &ctx.skill_name,
                skills_source_dir: &ctx.skill_dir,
                repo_root: &ctx.stage_root,
                harness: ctx.harness,
            })?;
        }
    }

    if let Some(warning) =
        staging_discovery_warning(ctx.harness, opts.no_stage, skills_dir_preexisted)
    {
        eprintln!("{warning}");
    }

    let bootstrap_content = match &ctx.bootstrap_path {
        Some(path) => Some(fs::read_to_string(path)?),
        None => None,
    };

    let plan_mode_content = if opts.plan_mode {
        let profile = resolve_plan_mode_profile(ctx.harness)?;
        println!(
            "  plan-mode: injecting {} plan-mode profile as operating context (issue #142; necessary-not-sufficient fidelity layer)",
            harness_label(ctx.harness)
        );
        Some(profile.to_string())
    } else {
        None
    };

    // Sibling skill metadata, shared across conditions. Empty when --no-stage.
    let sibling_skills: Vec<AvailableSkill> = if opts.no_stage {
        Vec::new()
    } else {
        ctx.sibling_skill_names
            .iter()
            .map(|name| AvailableSkill {
                name: name.clone(),
                path: skills_dir_for_harness(&ctx.stage_root, ctx.harness)
                    .join(name)
                    .join("SKILL.md")
                    .to_string_lossy()
                    .into_owned(),
                description: get_skill_description(&ctx.skill_dir.join(name).join("SKILL.md")),
            })
            .collect()
    };

    // --stage-name overrides the conspicuous slug with a verbatim name; it targets
    // the single staging condition, so reject the both-stage case and refuse to
    // clobber a pre-existing dir.
    if let Some(stage_name) = opts.stage_name
        && !opts.no_stage
    {
        if r.skill_path_a.is_some() && r.skill_path_b.is_some() {
            return Err(RunError::msg(
                "--stage-name is only supported when exactly one condition stages the skill (e.g. --mode new-skill); both conditions stage here.",
            ));
        }
        let target = skills_dir_for_harness(&ctx.stage_root, ctx.harness).join(stage_name);
        if target.exists() {
            return Err(RunError::msg(format!(
                "--stage-name \"{stage_name}\": {} already exists; refusing to clobber it. Remove it or choose a different name.",
                target.display()
            )));
        }
    }

    let stage_for =
        |cond_name: &str, cond_skill_path: Option<&str>| -> Result<Option<String>, RunError> {
            let Some(path) = cond_skill_path.filter(|_| !opts.no_stage) else {
                return Ok(None);
            };
            let content = fs::read_to_string(path)?;
            let slug = stage_skill_for_harness(&StageSkillOpts {
                content: &content,
                iteration: r.iteration,
                condition: cond_name,
                skill_name: &ctx.skill_name,
                repo_root: &ctx.stage_root,
                assets_dir: Path::new(path).parent(),
                stage_name_override: opts.stage_name,
                harness: ctx.harness,
            })?;
            Ok(Some(slug))
        };

    let cond_a_slug = stage_for(r.cond_a, r.skill_path_a.as_deref())?;
    let cond_b_slug = stage_for(r.cond_b, r.skill_path_b.as_deref())?;

    // A custom-named dir isn't caught by the prefix scan; record it for cleanup.
    if let Some(stage_name) = opts.stage_name
        && (cond_a_slug.as_deref() == Some(stage_name)
            || cond_b_slug.as_deref() == Some(stage_name))
    {
        register_staged_skill_for_cleanup(&ctx.stage_root, stage_name, ctx.harness)?;
    }

    Ok(Staged {
        cond_a_slug,
        cond_b_slug,
        sibling_skills,
        bootstrap_content,
        plan_mode_content,
        skills_dir_preexisted,
    })
}
