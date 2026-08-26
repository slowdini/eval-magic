//! Phase 2 — create the iteration dir, (re)stage the condition skills + their
//! siblings, and resolve the shared dispatch-prompt inputs.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::adapters::adapter_for;
use crate::core::RunContext;
use crate::core::fs::copy_entry_materialized;
use crate::sandbox::guard_profiles::{detect_profiles, expand_policy};
use crate::sandbox::teardown_guard;

use super::super::RunError;
use super::super::dispatch::get_skill_description;
use super::super::overlays::copy_overlay_files;
use super::super::staging::{
    StageSiblingOpts, StageSkillOpts, cleanup_staged_skills, exclude_codebase_skill_sources,
    register_staged_skill_for_cleanup, skills_dir_for_harness, stage_sibling_skills,
    stage_sibling_skills_excluding, stage_skill_for_harness,
};
use super::super::util::{harness_label, resolve_plan_mode_profile};
use super::envs::{EnvLayoutInput, env_targets};
use super::{Resolved, RunOptions, Staged, skills_copy_root};

pub(super) fn stage_conditions(
    ctx: &RunContext,
    opts: &RunOptions,
    r: &Resolved,
) -> Result<Staged, RunError> {
    fs::create_dir_all(&r.iteration_dir)?;
    // Before anything reads a skill: the copy every condition stages from. Made
    // even under `--no-stage`, where the dispatch prompt inlines the skill body
    // by reading the same path.
    let skills = materialize_skills(ctx, r)?;
    if !r.skill.multi {
        fs::copy(
            skills.join(&ctx.skill_name).join("SKILL.md"),
            r.iteration_dir.join("skill-snapshot.md"),
        )?;
    }

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
        r.skill
            .siblings
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    get_skill_description(&skills.join(name).join("SKILL.md")),
                )
            })
            .collect()
    };

    if opts.stage_name.is_some() && r.skill.multi {
        return Err(RunError::msg(
            "--stage-name is only supported for a single skill under test",
        ));
    }

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

    // The environments to stage: one per (group, condition), each with only its
    // condition's skill and its eval's overlay files.
    let targets = env_targets(&EnvLayoutInput {
        iteration_dir: &r.iteration_dir,
        groups: &r.groups,
        cond_a: r.cond_a,
        cond_b: r.cond_b,
        skill_path_a: r.skill_path_a.as_deref(),
        skill_path_b: r.skill_path_b.as_deref(),
    });

    let mut cond_a_slug = None;
    let mut cond_b_slug = None;
    let mut cond_a_skills = Vec::new();
    let mut cond_b_skills = Vec::new();
    // Distinct codebases materialized so far this iteration, by key. Every
    // environment sharing a codebase is provisioned from one materialization.
    let mut materialized: HashMap<String, PathBuf> = HashMap::new();
    let mut guard_policies = HashMap::new();
    let mut codebase_shadow_sources = HashMap::new();
    let evaluated_names = r
        .skill
        .treatments
        .iter()
        .map(|skill| skill.name.as_str())
        .chain(r.skill.siblings.iter().map(String::as_str))
        .collect::<Vec<_>>();

    for target in &targets {
        // Disarm a prior run's guard before re-staging, so a crashed run can't leave
        // the write-blocking hook armed across runs. Created unconditionally — even
        // under --no-stage, each env's overlays still land here.
        teardown_guard(&target.root);

        let codebase = r.codebase_for(&target.eval_ids)?;
        if target.root.exists() && adapter_for(ctx.harness).skills_dir(&target.root).is_some() {
            // Undo only runner-owned staging/exclusion from the reused environment
            // before it is reused or replaced. Never run this legacy cleanup
            // against a freshly provisioned codebase: it may legitimately own
            // a directory whose name uses the historical staging prefix.
            cleanup_staged_skills(&target.root, ctx.harness)?;
        }
        if target.root.exists() {
            // An explicit `--iteration N` rebuild would otherwise lay a fresh
            // codebase over the last run's tree, including whatever the previous
            // agent left behind. Start from nothing instead.
            fs::remove_dir_all(&target.root)?;
        }
        // The codebase goes down first: staged skills and the `files` overlay are
        // both applied *on top* of it. Every environment is provisioned from the
        // iteration's single cached materialization of it — a local clone while
        // the host allows the hard link, a plain copy otherwise — so `--runs 10`
        // against a real repository costs one checkout, not ten copies.
        let source_tree = materialize_codebase(&r.iteration_dir, codebase, &mut materialized)?;
        crate::source::provision_env(&codebase.source, &source_tree, &target.root)
            .map_err(|error| RunError::msg(error.to_string()))?;

        let inventoried = super::shadow_preflight::scan_codebase_skill_sources(
            &target.root,
            ctx.harness,
            &evaluated_names,
        );
        if codebase.declared.exclude_skill_sources() {
            exclude_codebase_skill_sources(&target.root, &ctx.skill_name, ctx.harness)?;
        } else if !inventoried.is_empty() {
            codebase_shadow_sources.insert(target.root.clone(), inventoried);
        }

        if !opts.no_stage && ctx.stage_siblings {
            let treatment_names = r
                .skill
                .treatments
                .iter()
                .map(|skill| skill.name.clone())
                .collect::<Vec<_>>();
            let sibling_opts = StageSiblingOpts {
                skill_under_test: &ctx.skill_name,
                skills_source_dir: &skills,
                repo_root: &target.root,
                harness: ctx.harness,
            };
            if treatment_names.len() == 1 {
                stage_sibling_skills(&sibling_opts)?;
            } else {
                stage_sibling_skills_excluding(&sibling_opts, &treatment_names)?;
            }
        }

        for (cond_name, _cond_skill_path) in &target.conditions {
            let condition_skills = if *cond_name == r.cond_a {
                &r.skill_paths_a
            } else {
                &r.skill_paths_b
            };
            // Refuse to clobber a pre-existing --stage-name dir in this env.
            if let Some(stage_name) = opts.stage_name
                && !opts.no_stage
                && !condition_skills.is_empty()
            {
                let dir = skills_dir_for_harness(&target.root, ctx.harness).join(stage_name);
                if dir.exists() {
                    return Err(RunError::msg(format!(
                        "--stage-name \"{stage_name}\": {} already exists; refusing to clobber it. Remove it or choose a different name.",
                        dir.display()
                    )));
                }
            }

            let mut staged_condition = Vec::new();
            for (skill_name, skill_path) in condition_skills {
                let slug = stage_for(
                    ctx,
                    opts,
                    r,
                    cond_name,
                    skill_name,
                    Some(skill_path),
                    &target.root,
                )?;
                if let Some(slug) = &slug
                    && opts.stage_name == Some(slug.as_str())
                {
                    register_staged_skill_for_cleanup(&target.root, slug, ctx.harness)?;
                }
                staged_condition.push(super::StagedTreatmentSkill {
                    name: skill_name.clone(),
                    slug,
                });
            }
            if *cond_name == r.cond_a {
                cond_a_skills = staged_condition;
                cond_a_slug = cond_a_skills
                    .iter()
                    .find(|skill| skill.name == ctx.skill_name)
                    .and_then(|skill| skill.slug.clone());
            } else {
                cond_b_skills = staged_condition;
                cond_b_slug = cond_b_skills
                    .iter()
                    .find(|skill| skill.name == ctx.skill_name)
                    .and_then(|skill| skill.slug.clone());
            }
        }

        // Apply this eval's authored files on top of the staged codebase.
        for eval_id in &target.eval_ids {
            if let Some(ev) = r.selected_evals.iter().find(|e| &e.id == eval_id) {
                copy_overlay_files(ev, &skills.join(&ctx.skill_name), &target.root)?;
            }
        }

        let eval = target
            .eval_ids
            .first()
            .and_then(|eval_id| r.selected_evals.iter().find(|eval| &eval.id == eval_id))
            .expect("canonical task environments contain one selected eval");
        let policy = match eval.guard.as_ref() {
            Some(policy) => expand_policy(policy),
            None => {
                let profiles = detect_profiles(&target.root)?;
                expand_policy(&crate::core::GuardPolicyConfig {
                    profiles,
                    ..crate::core::GuardPolicyConfig::default()
                })
            }
        }
        .map_err(|message| RunError::msg(format!("eval '{}': {message}", eval.id)))?;
        guard_policies.insert(target.root.clone(), policy);
    }

    Ok(Staged {
        cond_a_slug,
        cond_b_slug,
        cond_a_skills,
        cond_b_skills,
        sibling_meta,
        bootstrap_content,
        plan_mode_content,
        guard_policies,
        codebase_shadow_sources,
    })
}

/// Copy the skill under test and its recorded sibling roster into the iteration.
///
/// The resolver is not asked to materialize this: it would hand back a Git
/// repository, and what a skill needs is the working tree as it sits — the
/// uncommitted edit is usually the thing under test. The roster comes from the
/// resolution rather than a fresh scan, so what the artifacts claim and what the
/// environments hold cannot drift apart.
fn materialize_skills(ctx: &RunContext, r: &Resolved) -> Result<PathBuf, RunError> {
    let root = skills_copy_root(&r.iteration_dir);
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)?;
    for name in r
        .skill
        .treatments
        .iter()
        .map(|skill| &skill.name)
        .chain(r.skill.siblings.iter())
    {
        copy_skill_dir(&ctx.skill_dir.join(name), &root.join(name), &root)?;
    }
    Ok(root)
}

/// Copy one skill directory, minus its `.git` and minus whatever holds `root`.
///
/// A skill can be a repository root of its own; carrying the object store would
/// copy a history nothing here reads and that staging would then have to filter
/// out of every environment.
///
/// The `root` exclusion is what keeps the copy from swallowing itself.
/// `--workspace-dir` may legitimately point inside the skill tree — at
/// `.eval-magic` from inside a skill, say — and copying the directory the copy
/// is being written into recurses until the path length gives out. Testing
/// containment rather than matching a name covers every spelling the operator
/// can choose.
fn copy_skill_dir(source: &Path, dest: &Path, root: &Path) -> Result<(), RunError> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_name() == ".git" || root.starts_with(&path) {
            continue;
        }
        copy_entry_materialized(&path, &dest.join(entry.file_name()))?;
    }
    Ok(())
}

/// The materialized tree for `codebase`, creating it on first use.
///
/// One materialization per distinct codebase per iteration; every environment
/// is then provisioned from it ([`crate::source::provision_env`] — a local
/// clone while the host allows the hard link, a copy otherwise). Materializing
/// per environment instead would mean one network round trip per
/// `(group, condition, run)` cell.
fn materialize_codebase(
    iteration_dir: &Path,
    codebase: &super::RunCodebase,
    materialized: &mut HashMap<String, PathBuf>,
) -> Result<PathBuf, RunError> {
    if let Some(existing) = materialized.get(&codebase.key) {
        return Ok(existing.clone());
    }
    let tree = iteration_dir.join(".codebase").join(&codebase.key);
    if tree.exists() {
        fs::remove_dir_all(&tree)?;
    }
    crate::source::materialize(&codebase.source, &tree)
        .map_err(|error| RunError::msg(error.to_string()))?;
    materialized.insert(codebase.key.clone(), tree.clone());
    Ok(tree)
}

/// Stage one condition's skill into `root` and return its slug; `Ok(None)` when
/// the condition stages no skill (the new-skill control arm) or under --no-stage.
fn stage_for(
    ctx: &RunContext,
    opts: &RunOptions,
    r: &Resolved,
    cond_name: &str,
    skill_name: &str,
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
        skill_name,
        repo_root: root,
        assets_dir: Path::new(path).parent(),
        stage_name_override: opts.stage_name,
        harness: ctx.harness,
    })?;
    Ok(Some(slug))
}
