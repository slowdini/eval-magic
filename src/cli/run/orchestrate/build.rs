//! Phases 3 & 4 — build every `(eval, condition)` dispatch task and write
//! `conditions.json` / `dispatch-manifest.md` / per-task prompts / `dispatch.json`
//! ([`write_dispatch`]), then arm the write guard (auto-armed; `--no-guard`
//! opts out) and run the harness's shadow preflight ([`post_build`]).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::adapters::adapter_for;
use crate::core::{AvailableSkill, ConditionEntry, ConditionsRecord, RunContext};
use crate::pipeline::io::now_iso8601;

use super::super::dispatch::{
    DispatchTaskOpts, ManifestContext, build_dispatch_task, build_manifest, get_skill_description,
};
use super::super::fixtures::fixture_pairs;
use super::super::runbook::{RunbookContext, build_runbook};
use super::super::staging::skills_dir_for_harness;
use super::super::util::unguarded_notice;
use super::super::{RunError, write_json};
use super::envs::{EnvLayoutInput, env_targets, task_env_root_for_run, task_run_indices};
use super::{Resolved, RunOptions, Staged};
use crate::cli::command_target_args;

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

    let staged_skill_path_for = |env_root: &Path, cond_slug: Option<&str>| -> Option<String> {
        cond_slug.map(|slug| {
            skills_dir_for_harness(env_root, ctx.harness)
                .join(slug)
                .join("SKILL.md")
                .to_string_lossy()
                .into_owned()
        })
    };

    // availableSkills for a condition in a given env = siblings + the
    // skill-under-test when that condition loads it. Paths are task-env-specific.
    let available_skills_for = |env_root: &Path,
                                cond_skill_path: Option<&str>,
                                cond_slug: Option<&str>|
     -> Vec<AvailableSkill> {
        if opts.no_stage {
            return Vec::new();
        }
        let mut skills: Vec<AvailableSkill> = staged
            .sibling_meta
            .iter()
            .map(|(name, description)| AvailableSkill {
                name: name.clone(),
                path: skills_dir_for_harness(env_root, ctx.harness)
                    .join(name)
                    .join("SKILL.md")
                    .to_string_lossy()
                    .into_owned(),
                description: description.clone(),
            })
            .collect();
        if let Some(csp) = cond_skill_path {
            let name = match cond_slug {
                Some(slug) if adapter_for(ctx.harness).advertises_staged_slug_name() => {
                    slug.to_string()
                }
                _ => ctx.skill_name.clone(),
            };
            skills.push(AvailableSkill {
                name,
                path: staged_skill_path_for(env_root, cond_slug).unwrap_or_else(|| csp.to_string()),
                description: get_skill_description(Path::new(csp)),
            });
        }
        skills
    };

    // Each eval's env-relative fixture dests (for the task's `fixtures` field and
    // the prompt's fixtures block). The copies themselves are made per env by
    // `stage_conditions`; resolution here is read-only (and re-validated in resolve).
    let mut fixtures_by_eval: HashMap<&str, Vec<String>> = HashMap::new();
    for ev in &r.selected_evals {
        let dests = fixture_pairs(ev, &ctx.skill_subdir)?
            .into_iter()
            .map(|(dest, _source)| dest)
            .collect();
        fixtures_by_eval.insert(ev.id.as_str(), dests);
    }

    // A single group keeps the `group` key off each task (>1 group tags them);
    // `eval_root` (the private per-task cwd) is always set.
    let multi_group = r.groups.len() > 1;

    let mut tasks = Vec::new();
    // Build tasks CONDITION-outer, GROUP-inner. A single group collapses this to
    // the legacy condition-outer order.
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
        for group in &r.groups {
            for eval_id in &group.eval_ids {
                let ev = r
                    .selected_evals
                    .iter()
                    .find(|e| &e.id == eval_id)
                    .expect("group eval ids are drawn from selected_evals");
                let cond_dir = r
                    .iteration_dir
                    .join(format!("eval-{}", ev.id))
                    .join(cond_name);
                let runs = ev.runs.unwrap_or(opts.runs);

                for run_idx in 1..=runs {
                    // A single-run cell keeps the flat legacy layout; multi-run
                    // cells nest each run under run-<k>/.
                    let (run_dir, run_index) = if runs == 1 {
                        (cond_dir.clone(), None)
                    } else {
                        (cond_dir.join(format!("run-{run_idx}")), Some(run_idx))
                    };
                    let env_run_index = group
                        .task_runs
                        .is_some_and(|task_runs| task_runs > 1)
                        .then_some(run_idx);
                    let env_root = task_env_root_for_run(
                        &r.iteration_dir,
                        &group.id,
                        cond_name,
                        env_run_index,
                    );
                    let env_root_str = env_root.to_string_lossy().into_owned();
                    let staged_path = staged_skill_path_for(&env_root, cond_slug);
                    let available_skills =
                        available_skills_for(&env_root, cond_skill_path, cond_slug);
                    // Create the per-run meta dir (run.json / timing.json), which
                    // lives above the env.
                    fs::create_dir_all(&run_dir)?;
                    // The agent-under-test's cwd is its env, so its outputs land
                    // *inside* it — never above its sandbox. A hidden,
                    // per-(eval, condition, run) subtree keeps concurrent same-env
                    // subagents from colliding.
                    let outputs_rel = match run_index {
                        None => format!("eval-{}/{cond_name}", ev.id),
                        Some(k) => format!("eval-{}/{cond_name}/run-{k}", ev.id),
                    };
                    let outputs_dir = env_root.join(".eval-magic-outputs").join(outputs_rel);
                    fs::create_dir_all(&outputs_dir)?;

                    let fixtures = fixtures_by_eval
                        .get(ev.id.as_str())
                        .cloned()
                        .unwrap_or_default();
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
                        available_skills: available_skills.clone(),
                        harness: ctx.harness,
                        run_tag: Some(&r.run_tag),
                        run_index,
                        // Tag the group only when there's more than one (keeps the
                        // single-group task byte-identical). `eval_root` is the
                        // per-task cwd the CLI recipe `cd`s into.
                        group: multi_group.then_some(group.id.as_str()),
                        eval_root: Some(env_root_str.as_str()),
                    })?);
                }
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
            ManifestContext {
                harness: ctx.harness,
                guard: opts.guard_armed(),
                agent_model: opts.agent_model,
            },
        ),
    )?;

    // Write each prompt to its own file; dispatch.json references it by path.
    for task in &tasks {
        fs::write(&task.dispatch_prompt_path, &task.dispatch_prompt)?;
    }

    let dispatch_json_path = r.iteration_dir.join("dispatch.json");
    let mut dispatch_json = json!({
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
    // The isolation-batch plan the executing session/human follows: which evals
    // share an env, why, and (per condition) the env each batch runs in. There is
    // one env per (group, condition).
    let groups: Vec<Value> = r
        .groups
        .iter()
        .map(|g| {
            let envs: Vec<Value> = [r.cond_a, r.cond_b]
                .iter()
                .flat_map(|cond| {
                    task_run_indices(g).into_iter().map(move |run_index| {
                        let mut env = json!({
                            "condition": cond,
                            "dir": task_env_root_for_run(
                                &r.iteration_dir,
                                &g.id,
                                cond,
                                run_index,
                            )
                            .to_string_lossy(),
                        });
                        if let Some(run_index) = run_index {
                            env.as_object_mut()
                                .expect("env summary is an object")
                                .insert("run_index".to_string(), json!(run_index));
                        }
                        env
                    })
                })
                .collect();
            json!({
                "id": g.id,
                "evals": g.eval_ids,
                "rationale": g.rationale,
                "envs": envs,
            })
        })
        .collect();
    dispatch_json
        .as_object_mut()
        .expect("dispatch_json is a JSON object")
        .insert("groups".to_string(), Value::Array(groups));
    write_json(&dispatch_json_path, &dispatch_json)?;

    // The followable handoff artifact: a human reads RUNBOOK.md to run the loop.
    // It references eval-magic meta (dispatch.json, benchmark.json) under
    // `iteration_dir`, so `RunbookContext` keeps `iteration_dir`, not the env, and
    // the human drives from there. Generated, not version controlled.
    let target_args = command_target_args(ctx);
    let runbook = build_runbook(&RunbookContext {
        harness: ctx.harness,
        skill_name: &ctx.skill_name,
        iteration: r.iteration,
        iteration_dir: &r.iteration_dir,
        mode: r.mode,
        cond_a: r.cond_a,
        cond_b: r.cond_b,
        num_tasks: tasks.len(),
        target_args: &target_args,
        guard: opts.guard_armed(),
        agent_model: opts.agent_model,
    });
    fs::write(r.iteration_dir.join("RUNBOOK.md"), runbook)?;

    Ok(tasks.len())
}

/// Post-build side effects: arm the write guard (auto-armed when the harness
/// declares one; `--no-guard` opts out) and run the harness's shadow
/// preflight.
pub(super) fn post_build(
    ctx: &RunContext,
    opts: &RunOptions,
    r: &Resolved,
) -> Result<(), RunError> {
    // Every private task env this run staged. Computed once and
    // reused below to arm the guard in each env and to point the plugin-shadow
    // preflight at a real staged env.
    let targets = env_targets(&EnvLayoutInput {
        iteration_dir: &r.iteration_dir,
        groups: &r.groups,
        cond_a: r.cond_a,
        cond_b: r.cond_b,
        skill_path_a: r.skill_path_a.as_deref(),
        skill_path_b: r.skill_path_b.as_deref(),
    });

    // Hard guard: a PreToolUse hook blocking subagent writes/installs outside
    // the eval sandbox while dispatches run. Armed in *every* env the run
    // staged — since each subprocess loads its hook from its own cwd. The
    // preflight resolved the guard state (no-stage and guardless-harness runs
    // arrive with it off), so only --dry-run gates the install here.
    if opts.guard_armed() && !opts.dry_run {
        let adapter = adapter_for(ctx.harness);
        let exe = std::env::current_exe()?;
        for target in &targets {
            adapter.install_guard(&target.root, &exe, None)?;
        }
        if let Some(msg) = adapter.guard_armed_message() {
            println!("{msg}");
        }
    }

    // No-stage runs can't arm the guard at all — say so in the summary, whether
    // or not --guard was passed, so the operator knows the run is unguarded.
    if !opts.dry_run
        && let Some(notice) = unguarded_notice(opts.no_stage)
    {
        eprintln!("{notice}");
    }

    // Shadow preflight: a staged skill name also discoverable from the operator's
    // live environment contaminates the run. Scan the first staged env, not
    // `ctx.stage_root` — project-local settings must be read from a real staged
    // task env.
    let mut names: Vec<&str> = vec![ctx.skill_name.as_str()];
    names.extend(ctx.sibling_skill_names.iter().map(String::as_str));
    let scan_root = targets
        .first()
        .map(|t| t.root.as_path())
        .unwrap_or(ctx.stage_root.as_path());
    let adapter = adapter_for(ctx.harness);
    if let Some(report) = adapter.detect_shadowed_skills(scan_root, &names) {
        write_json(&r.iteration_dir.join("plugin-shadow.json"), &report)?;
        eprintln!("{}", adapter.format_shadow_banner(&report));
    }
    crate::pipeline::capture_iteration_baselines(&r.iteration_dir)
        .map_err(|error| RunError::msg(error.to_string()))?;
    Ok(())
}
