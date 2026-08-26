//! Phase 1 — validate the request and resolve the iteration coordinates +
//! per-condition skill paths, before any directory is created.

use std::fs;
use std::path::{Component, Path};

use serde_json::Value;

use crate::cli::command_target_args;
use crate::core::{Assertion, CodebaseSource, Eval, EvalsConfig, Mode, RunContext};
use crate::source::{SourceSpec, resolve as resolve_source};
use crate::validation::validate_evals_config;

use super::super::RunError;
use super::super::dispatch::select_evals;
use super::super::grouping::{GroupInput, compute_groups};
use super::super::overlays::{overlay_file_pairs, setup_file_pairs};
use super::super::util::{condition_names_for, make_run_nonce, next_iteration};
use super::{Resolved, RunCodebase, RunOptions, RunSkill, skills_copy_root};

/// Resolve every distinct codebase the selected evals declare, deduplicated so
/// a config-level default shared by ten evals is one resolution and, later, one
/// materialization.
///
/// The `CodebaseSource` → `SourceSpec` translation lives here rather than as a
/// `From` impl in [`crate::source`]: that module resolves the skill under test
/// as well, and stays useful precisely because it does not know what a codebase is.
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
        let declared = eval
            .codebase
            .as_ref()
            .or(config.codebase.as_ref())
            .expect("validated eval has an effective codebase");
        if let Some(existing) = codebases
            .iter_mut()
            .find(|candidate| &candidate.declared == declared)
        {
            existing.eval_ids.push(eval.id.clone());
            continue;
        }

        let spec = match declared {
            CodebaseSource::Git { url, reference, .. } => SourceSpec::Git {
                url: url.clone(),
                reference: reference.clone(),
            },
            CodebaseSource::Path { path, .. } => SourceSpec::Path { path: path.clone() },
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

    let evals_path = ctx.skill_subdir.join("evals").join("evals.json");
    if !evals_path.exists() {
        return Err(RunError::msg(format!(
            "evals.json not found: {}",
            evals_path.display()
        )));
    }
    let value: Value = serde_json::from_str(&fs::read_to_string(&evals_path)?)?;
    let config = validate_evals_config(&value, &evals_path.to_string_lossy())?;
    if config.skill_name.is_multi() && !config.skill_names().contains(&ctx.skill_name) {
        return Err(RunError::msg(format!(
            "eval owner '{}' must be listed in skill_name",
            ctx.skill_name
        )));
    }
    if !config.skill_name.is_multi() && config.skill_names().first() != Some(&ctx.skill_name) {
        eprintln!(
            "warning: evals.json skill_name ({}) does not match the skill folder ({}). Proceeding with {}.",
            config.skill_name, ctx.skill_name, ctx.skill_name
        );
    }

    // A scalar config retains the historical "selected folder wins" behavior.
    // A list is an explicit treatment roster, so every authored member resolves
    // from the selected skills root in its authored order.
    let treatment_names = if config.skill_name.is_multi() {
        config.skill_names().to_vec()
    } else {
        vec![ctx.skill_name.clone()]
    };
    let mut treatments = Vec::with_capacity(treatment_names.len());
    for name in &treatment_names {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
            || name.contains('\\')
        {
            return Err(RunError::msg(format!(
                "invalid skill_name member '{name}': expected one skill directory name"
            )));
        }
        let path = ctx.skill_dir.join(name);
        let skill_md_path = path.join("SKILL.md");
        if !skill_md_path.exists() {
            return Err(RunError::msg(format!(
                "skill not found: {}",
                skill_md_path.display()
            )));
        }
        let source = resolve_source(
            &SourceSpec::Path {
                path: path.to_string_lossy().into_owned(),
            },
            &ctx.skill_dir,
            "skill",
        )
        .map_err(|error| RunError::msg(error.to_string()))?;
        treatments.push(super::TreatmentSkill {
            name: name.clone(),
            source,
        });
    }
    let owner_source = treatments
        .iter()
        .find(|skill| skill.name == ctx.skill_name)
        .expect("scalar owner or validated multi owner")
        .source
        .clone();
    let skill = RunSkill {
        eval_owner: ctx.skill_name.clone(),
        multi: config.skill_name.is_multi(),
        source: owner_source,
        treatments,
        // The ambient roster is what remains after removing every treatment
        // member from the selected skills directory.
        siblings: if ctx.stage_siblings && !opts.no_stage {
            ctx.sibling_skill_names
                .iter()
                .filter(|name| !treatment_names.contains(name))
                .cloned()
                .collect()
        } else {
            Vec::new()
        },
    };

    let selected_evals = select_evals(&config.evals, opts.only, opts.skip)?
        .into_iter()
        .map(|mut eval| {
            eval.guard = config.guard_for(&eval).cloned();
            eval
        })
        .collect::<Vec<_>>();
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

    // Resolve overlay sources before any environment is staged, so a missing
    // declared file fails without leaving a partial iteration behind.
    for eval in &selected_evals {
        overlay_file_pairs(eval, &ctx.skill_subdir)?;
    }
    let group_inputs: Vec<GroupInput> = selected_evals
        .iter()
        .map(|ev| GroupInput {
            eval_id: &ev.id,
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
    let copied_skill_paths = skill
        .treatments
        .iter()
        .map(|treatment| {
            (
                treatment.name.clone(),
                skills_copy_root(&iteration_dir)
                    .join(&treatment.name)
                    .join("SKILL.md")
                    .to_string_lossy()
                    .into_owned(),
            )
        })
        .collect::<Vec<_>>();
    let copied_owner_skill_md = copied_skill_paths
        .iter()
        .find(|(name, _)| name == &ctx.skill_name)
        .map(|(_, path)| path.clone())
        .expect("the eval owner belongs to the treatment");
    let (skill_paths_a, skill_paths_b) = match mode {
        Mode::NewSkill => (copied_skill_paths.clone(), Vec::new()),
        Mode::Revision => {
            let baseline = baseline.as_deref().expect("revision baseline set above");
            let baseline_root = workspace_skill_dir.join("snapshots").join(baseline);
            let baseline_paths = skill
                .treatments
                .iter()
                .map(|treatment| {
                    let path = if skill.multi {
                        baseline_root
                            .join("skills")
                            .join(&treatment.name)
                            .join("SKILL.md")
                    } else {
                        baseline_root.join("SKILL.md")
                    };
                    if !path.exists() {
                        let target_args = command_target_args(ctx);
                        return Err(RunError::msg(format!(
                            "baseline snapshot not found: {}\n  Run: eval-magic snapshot{target_args} --label {} (before editing)",
                            path.display(),
                            baseline
                        )));
                    }
                    Ok((treatment.name.clone(), path.to_string_lossy().into_owned()))
                })
                .collect::<Result<Vec<_>, RunError>>()?;
            (baseline_paths, copied_skill_paths.clone())
        }
    };
    let owner_path = |paths: &[(String, String)]| {
        paths
            .iter()
            .find(|(name, _)| name == &ctx.skill_name)
            .map(|(_, path)| path.clone())
    };
    let skill_path_a = owner_path(&skill_paths_a);
    let skill_path_b = owner_path(&skill_paths_b);
    if mode == Mode::NewSkill {
        debug_assert_eq!(
            skill_path_a.as_deref(),
            Some(copied_owner_skill_md.as_str())
        );
    }

    // The mirror image of the codebase warning: a skill is copied as it sits, so
    // uncommitted work is in what ran, and the recorded revision alone does not
    // name it.
    for treatment in skill.treatments.iter().filter(|skill| skill.source.dirty) {
        eprintln!(
            "⚠ skill '{}' has uncommitted changes; the run measures them, so its recorded \
             revision alone does not identify what was evaluated",
            treatment.name
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
        skill_paths_a,
        skill_paths_b,
        selected_evals,
        total_evals,
        groups,
    })
}
