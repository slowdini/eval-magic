//! Phase 1 — validate the request and resolve the iteration coordinates +
//! per-condition skill paths, before any directory is created.

use std::fs;

use serde_json::Value;

use crate::cli::command_target_args;
use crate::core::{Assertion, CodebaseSource, Eval, EvalsConfig, Mode, RunContext};
use crate::source::{SourceSpec, resolve as resolve_source};
use crate::validation::validate_evals_config;

use super::super::RunError;
use super::super::dispatch::select_evals;
use super::super::fixtures::{fixture_pairs, setup_file_pairs};
use super::super::grouping::{GroupInput, compute_groups};
use super::super::util::{condition_names_for, make_run_nonce, next_iteration};
use super::{Resolved, RunCodebase, RunOptions, RunSkill, skills_copy_root};

/// Resolve every distinct codebase the selected evals declare, deduplicated so
/// a config-level default shared by ten evals is one resolution and, later, one
/// materialization.
///
/// The `CodebaseSource` → `SourceSpec` translation lives here rather than as a
/// `From` impl in [`crate::source`]: that module resolves skills for #253 too,
/// and stays useful precisely because it does not know what a codebase is.
fn resolve_codebases(
    ctx: &RunContext,
    config: &EvalsConfig,
    selected: &[Eval],
) -> Result<Vec<RunCodebase>, RunError> {
    // A declared relative path is relative to the config that declares it, so a
    // committed `evals.json` means the same thing in every clone of the skill.
    let base_dir = ctx.skill_subdir.join("evals");
    let mut codebases: Vec<RunCodebase> = Vec::new();

    for eval in selected {
        let Some(declared) = eval.codebase.as_ref().or(config.codebase.as_ref()) else {
            continue;
        };
        if let Some(existing) = codebases
            .iter_mut()
            .find(|candidate| &candidate.declared == declared)
        {
            existing.eval_ids.push(eval.id.clone());
            continue;
        }

        let spec = match declared {
            CodebaseSource::Git { url, reference } => SourceSpec::Git {
                url: url.clone(),
                reference: reference.clone(),
            },
            CodebaseSource::Path { path } => SourceSpec::Path { path: path.clone() },
        };
        let source = resolve_source(&spec, &base_dir, "codebase")
            .map_err(|error| RunError::msg(format!("eval '{}': {error}", eval.id)))?;
        // Keyed on the resolved commit so two evals naming the same tree by
        // different refs still materialize once. A directory with no history has
        // no commit to key on and falls back to declaration order.
        let key = source
            .revision
            .clone()
            .unwrap_or_else(|| format!("local-{}", codebases.len() + 1));
        codebases.push(RunCodebase {
            declared: declared.clone(),
            source,
            key,
            eval_ids: vec![eval.id.clone()],
        });
    }
    Ok(codebases)
}

pub(super) fn resolve_request(ctx: &RunContext, opts: &RunOptions) -> Result<Resolved, RunError> {
    let mode = match opts.mode {
        Some("new-skill") => Mode::NewSkill,
        Some("revision") => Mode::Revision,
        Some(other) => return Err(RunError::msg(format!("unknown --mode: {other}"))),
        None => Mode::NewSkill,
    };
    let baseline =
        (mode == Mode::Revision).then(|| opts.baseline.unwrap_or("baseline").to_string());
    if opts.runs == 0 {
        return Err(RunError::msg("--runs must be at least 1"));
    }

    let skill_md_path = ctx.skill_subdir.join("SKILL.md");
    if !skill_md_path.exists() {
        return Err(RunError::msg(format!(
            "skill not found: {}",
            skill_md_path.display()
        )));
    }

    // Resolve the skill as a source, the same way a codebase is: this is what
    // gives a report a revision to cite on the skill side. Read-only, like every
    // resolution here — the copy itself is taken during staging.
    let skill_source = resolve_source(
        &SourceSpec::Path {
            path: ctx.skill_subdir.to_string_lossy().into_owned(),
        },
        &ctx.skill_dir,
        "skill",
    )
    .map_err(|error| RunError::msg(error.to_string()))?;
    let skill = RunSkill {
        source: skill_source,
        siblings: if ctx.stage_siblings {
            ctx.sibling_skill_names.clone()
        } else {
            Vec::new()
        },
    };

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

    // Resolve declared codebases here, while the run has still created nothing:
    // an unreachable repository or a ref that does not exist has to fail before
    // any environment exists, not halfway through building one.
    let codebases = resolve_codebases(ctx, &config, &selected_evals)?;
    // A codebase is materialized as a clean checkout of its committed state, so
    // uncommitted work in the source is silently absent from the environment.
    // Saying so is what keeps that from being a surprise.
    for codebase in codebases.iter().filter(|c| c.source.dirty) {
        eprintln!(
            "⚠ codebase '{}' has uncommitted changes; the task environment is a clean checkout \
             of its committed state and does not include them",
            codebase.source.source
        );
    }

    // Resolve held-out setup sources before creating the iteration. The files
    // are deliberately not copied here; command grading injects them during
    // ingest after the agent has finished.
    for ev in &selected_evals {
        for assertion in ev.assertions.as_deref().unwrap_or(&[]) {
            if let Assertion::CommandCheck(check) = assertion {
                setup_file_pairs(check, &ctx.skill_subdir)?;
            }
        }
    }

    // Compute isolation groups up front (fixture-conflict + explicit hint), before
    // any env is staged: staging and dispatch both consume this plan. `fixture_pairs`
    // also fails fast here if a declared fixture is missing.
    let fixture_pairs_by_eval = selected_evals
        .iter()
        .map(|ev| fixture_pairs(ev, &ctx.skill_subdir))
        .collect::<Result<Vec<_>, _>>()?;
    let group_inputs: Vec<GroupInput> = selected_evals
        .iter()
        .zip(&fixture_pairs_by_eval)
        .map(|(ev, fixtures)| GroupInput {
            eval_id: &ev.id,
            isolation: ev.isolation,
            fixtures,
            // Every canonical run publishes final-environment diff metrics, so
            // each eval/run needs a private environment.
            task_scoped: true,
            runs: ev.runs.unwrap_or(opts.runs),
        })
        .collect();
    let groups = compute_groups(&group_inputs);

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
    // Conditions stage from the copy the eval home will hold, never from the
    // operator's tree. The copy does not exist yet; `stage_conditions` creates it
    // before anything reads these paths.
    let copied_skill_md = skills_copy_root(&iteration_dir)
        .join(&ctx.skill_name)
        .join("SKILL.md")
        .to_string_lossy()
        .into_owned();
    let (skill_path_a, skill_path_b): (Option<String>, Option<String>) = match mode {
        Mode::NewSkill => (Some(copied_skill_md.clone()), None),
        Mode::Revision => {
            let baseline = baseline.as_deref().expect("revision baseline set above");
            let baseline_skill = workspace_skill_dir
                .join("snapshots")
                .join(baseline)
                .join("SKILL.md");
            if !baseline_skill.exists() {
                let target_args = command_target_args(ctx);
                return Err(RunError::msg(format!(
                    "baseline snapshot not found: {}\n  Run: eval-magic snapshot{target_args} --label {} (before editing)",
                    baseline_skill.display(),
                    baseline
                )));
            }
            (
                Some(baseline_skill.to_string_lossy().into_owned()),
                Some(copied_skill_md.clone()),
            )
        }
    };

    // The mirror image of the codebase warning: a skill is copied as it sits, so
    // uncommitted work is in what ran, and the recorded revision alone does not
    // name it.
    if skill.source.dirty {
        eprintln!(
            "⚠ skill '{}' has uncommitted changes; the run measures them, so its recorded \
             revision alone does not identify what was evaluated",
            ctx.skill_name
        );
    }

    Ok(Resolved {
        mode,
        baseline,
        codebases,
        skill,
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
        groups,
    })
}
