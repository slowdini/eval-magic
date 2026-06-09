//! Phase 1 — validate the request and resolve the iteration coordinates +
//! per-condition skill paths, before any directory is created.
//! Ports `run.ts:567-647`.

use std::fs;

use serde_json::Value;

use crate::core::{Mode, RunContext};
use crate::validation::validate_evals_config;

use super::super::RunError;
use super::super::dispatch::select_evals;
use super::super::util::{
    condition_names_for, make_run_nonce, next_iteration, validate_harness_run_options,
};
use super::{Resolved, RunOptions};

pub(super) fn resolve_request(ctx: &RunContext, opts: &RunOptions) -> Result<Resolved, RunError> {
    let mode = match opts.mode {
        Some("new-skill") => Mode::NewSkill,
        Some("revision") => Mode::Revision,
        Some(other) => return Err(RunError::msg(format!("unknown --mode: {other}"))),
        None => return Err(RunError::msg("--mode required: new-skill | revision")),
    };
    if mode == Mode::Revision && opts.baseline.is_none() {
        return Err(RunError::msg("revision mode requires --baseline <label>"));
    }
    validate_harness_run_options(opts, ctx)?;

    let skill_md_path = ctx.skill_subdir.join("SKILL.md");
    if !skill_md_path.exists() {
        return Err(RunError::msg(format!(
            "skill not found: {}",
            skill_md_path.display()
        )));
    }
    let skill_md = skill_md_path.to_string_lossy().into_owned();

    let evals_path = ctx.skill_subdir.join("evals").join("evals.json");
    if !evals_path.exists() {
        return Err(RunError::msg(format!(
            "evals.json not found: {}",
            evals_path.display()
        )));
    }
    let value: Value = serde_json::from_str(&fs::read_to_string(&evals_path)?)?;
    let config = validate_evals_config(&value, &evals_path.to_string_lossy())?;
    if config.skill_name != ctx.skill_name {
        eprintln!(
            "warning: evals.json skill_name ({}) does not match the skill folder ({}). Proceeding with {}.",
            config.skill_name, ctx.skill_name, ctx.skill_name
        );
    }

    let selected_evals = select_evals(&config.evals, opts.only, opts.skip)?;
    let total_evals = config.evals.len();

    let workspace_skill_dir = ctx.workspace_root.join(&ctx.skill_name);
    let iteration = next_iteration(&workspace_skill_dir, opts.iteration);
    let iteration_dir = workspace_skill_dir.join(format!("iteration-{iteration}"));
    let run_nonce = make_run_nonce();
    let run_tag = format!("i{iteration}-{run_nonce}");

    if iteration_dir.exists() && opts.iteration.is_none() {
        return Err(RunError::msg(format!(
            "iteration-{iteration} already exists; pass --iteration to overwrite explicitly"
        )));
    }

    let (cond_a, cond_b) = condition_names_for(mode);
    let (skill_path_a, skill_path_b): (Option<String>, Option<String>) = match mode {
        Mode::NewSkill => (Some(skill_md.clone()), None),
        Mode::Revision => {
            let baseline = opts.baseline.expect("checked above");
            let baseline_skill = workspace_skill_dir
                .join("snapshots")
                .join(baseline)
                .join("SKILL.md");
            if !baseline_skill.exists() {
                return Err(RunError::msg(format!(
                    "baseline snapshot not found: {}\n  Run: skill-eval snapshot --skill {} --skill-dir {} --label {} (before editing)",
                    baseline_skill.display(),
                    ctx.skill_name,
                    ctx.skill_dir.display(),
                    baseline
                )));
            }
            (
                Some(baseline_skill.to_string_lossy().into_owned()),
                Some(skill_md.clone()),
            )
        }
    };

    Ok(Resolved {
        mode,
        skill_md_path,
        iteration,
        iteration_dir,
        run_nonce,
        run_tag,
        cond_a,
        cond_b,
        skill_path_a,
        skill_path_b,
        selected_evals,
        total_evals,
    })
}
