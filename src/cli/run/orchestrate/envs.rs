//! Env-layout planning: turn the computed isolation [`Group`]s into the concrete
//! environment directories a run stages into.
//!
//! A canonical run gives every `(eval, condition, run)` dispatch a private
//! `iteration-N/env-<group>-<condition>[-run-<k>]/`. Each subprocess `cd`s into
//! its own env, which holds only that condition's skill (or none) and that eval's
//! fixtures.

use std::path::{Path, PathBuf};

use super::super::grouping::Group;

/// One environment directory to stage for a run.
pub(super) struct EnvTarget {
    pub root: PathBuf,
    /// `(condition name, that condition's skill path)` staged into this env —
    /// exactly one per env.
    pub conditions: Vec<(&'static str, Option<String>)>,
    /// Eval ids whose fixtures populate this env (its group's evals).
    pub eval_ids: Vec<String>,
}

/// Inputs to [`env_targets`].
pub(super) struct EnvLayoutInput<'a> {
    pub iteration_dir: &'a Path,
    pub groups: &'a [Group],
    pub cond_a: &'static str,
    pub cond_b: &'static str,
    pub skill_path_a: Option<&'a str>,
    pub skill_path_b: Option<&'a str>,
}

/// The base env dir a task-scoped `(group, condition)` task runs in.
pub(super) fn task_env_root(iteration_dir: &Path, group_id: &str, condition: &str) -> PathBuf {
    iteration_dir.join(format!("env-{group_id}-{condition}"))
}

/// The env dir for a task-scoped run. Single-run tasks retain the historical
/// group/condition name; multi-run tasks append `-run-<k>`.
pub(super) fn task_env_root_for_run(
    iteration_dir: &Path,
    group_id: &str,
    condition: &str,
    run_index: Option<u32>,
) -> PathBuf {
    let base = task_env_root(iteration_dir, group_id, condition);
    match run_index {
        Some(k) => PathBuf::from(format!("{}-run-{k}", base.to_string_lossy())),
        None => base,
    }
}

pub(super) fn task_run_indices(group: &Group) -> Vec<Option<u32>> {
    match group.task_runs {
        Some(runs) if runs > 1 => (1..=runs).map(Some).collect(),
        _ => vec![None],
    }
}

/// Plan the private environments to stage.
pub(super) fn env_targets(input: &EnvLayoutInput) -> Vec<EnvTarget> {
    let conds: [(&'static str, Option<String>); 2] = [
        (input.cond_a, input.skill_path_a.map(str::to_owned)),
        (input.cond_b, input.skill_path_b.map(str::to_owned)),
    ];
    input
        .groups
        .iter()
        .flat_map(|g| {
            conds.clone().into_iter().flat_map(move |(cond, skill)| {
                task_run_indices(g)
                    .into_iter()
                    .map(move |run_index| EnvTarget {
                        root: task_env_root_for_run(input.iteration_dir, &g.id, cond, run_index),
                        conditions: vec![(cond, skill.clone())],
                        eval_ids: g.eval_ids.clone(),
                    })
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups() -> Vec<Group> {
        vec![
            Group {
                id: "g1".into(),
                eval_ids: vec!["e1".into()],
                rationale: "default".into(),
                task_runs: None,
            },
            Group {
                id: "g2".into(),
                eval_ids: vec!["e2".into()],
                rationale: "fixture-conflict: e2 vs e1 at c.json".into(),
                task_runs: None,
            },
        ]
    }

    #[test]
    fn one_env_per_group_condition_with_only_that_conditions_skill() {
        let iter = Path::new("/w/iteration-1");
        let gs = groups();
        let targets = env_targets(&EnvLayoutInput {
            iteration_dir: iter,
            groups: &gs,
            cond_a: "with_skill",
            cond_b: "without_skill",
            skill_path_a: Some("/s/SKILL.md"),
            skill_path_b: None,
        });
        assert_eq!(targets.len(), 4, "2 groups × 2 conditions");
        let roots: Vec<String> = targets
            .iter()
            .map(|t| t.root.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            roots,
            vec![
                "/w/iteration-1/env-g1-with_skill",
                "/w/iteration-1/env-g1-without_skill",
                "/w/iteration-1/env-g2-with_skill",
                "/w/iteration-1/env-g2-without_skill",
            ]
        );
        // The with_skill env carries the skill; the control arm's env carries none.
        let with = &targets[0];
        assert_eq!(
            with.conditions,
            vec![("with_skill", Some("/s/SKILL.md".to_string()))]
        );
        let without = &targets[1];
        assert_eq!(without.conditions, vec![("without_skill", None)]);
        // Each env only holds its group's evals.
        assert_eq!(targets[0].eval_ids, vec!["e1"]);
        assert_eq!(targets[2].eval_ids, vec!["e2"]);
    }

    #[test]
    fn task_env_root_is_suffixed_by_group_and_condition() {
        let iter = Path::new("/w/iteration-1");
        assert_eq!(
            task_env_root(iter, "g2", "without_skill"),
            Path::new("/w/iteration-1/env-g2-without_skill")
        );
    }

    #[test]
    fn task_scoped_multi_run_group_gets_one_env_per_condition_and_run() {
        let iter = Path::new("/w/iteration-1");
        let groups = vec![Group {
            id: "g1".into(),
            eval_ids: vec!["held-out".into()],
            rationale: "metric: diff_scope".into(),
            task_runs: Some(2),
        }];
        let targets = env_targets(&EnvLayoutInput {
            iteration_dir: iter,
            groups: &groups,
            cond_a: "with_skill",
            cond_b: "without_skill",
            skill_path_a: Some("/s/SKILL.md"),
            skill_path_b: None,
        });
        assert_eq!(
            targets
                .iter()
                .map(|target| target.root.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec![
                "/w/iteration-1/env-g1-with_skill-run-1",
                "/w/iteration-1/env-g1-with_skill-run-2",
                "/w/iteration-1/env-g1-without_skill-run-1",
                "/w/iteration-1/env-g1-without_skill-run-2",
            ]
        );
    }
}
