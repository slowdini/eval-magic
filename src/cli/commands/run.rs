//! `run` and `dispatch-task` handlers.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::anyhow;

use crate::adapters::adapter_for;
use crate::cli::args::{DispatchTaskArgs, RunArgs};
use crate::cli::run;
use crate::cli::{parse_id_list, run_context_with_bootstrap};
use crate::core::validate_agent_environment_entry;

fn parse_agent_environment(values: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
    let mut environment = BTreeMap::new();
    for assignment in values {
        let (name, value) = assignment
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid --agent-env {assignment:?}: expected KEY=VALUE"))?;
        validate_agent_environment_entry(name, value)
            .map_err(|message| anyhow!("invalid --agent-env {assignment:?}: {message}"))?;
        environment.insert(name.to_string(), value.to_string());
    }
    Ok(environment)
}

/// Build the iteration workspace and dispatch plan (the default action).
pub(crate) fn run_run(args: RunArgs) -> anyhow::Result<()> {
    let cli_agent_env = parse_agent_environment(&args.agent_env)?;
    let ctx = run_context_with_bootstrap(&args.common, args.bootstrap.clone())?;
    let mut agent_env = adapter_for(ctx.harness).dispatch_environment();
    agent_env.extend(cli_agent_env);
    let only = parse_id_list(args.common.only.as_deref());
    let skip = parse_id_list(args.common.skip.as_deref());
    run::orchestrate::command_run(
        &ctx,
        &run::orchestrate::RunOptions {
            mode: args.common.mode.as_deref(),
            baseline: args.baseline.as_deref(),
            only: only.as_deref(),
            skip: skip.as_deref(),
            iteration: args.common.iteration,
            dry_run: args.dry_run,
            no_stage: args.no_stage,
            guard: match (args.guard, args.no_guard) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            },
            stage_name: args.stage_name.as_deref(),
            plan_mode: args.plan_mode,
            runs: args.runs,
            agent_model: args.agent_model.as_deref(),
            agent_env,
            judge_model: args.judge_model.as_deref(),
            label: args.label.as_deref(),
        },
    )?;
    Ok(())
}

pub(crate) fn run_dispatch_task(args: DispatchTaskArgs) -> anyhow::Result<()> {
    run::conversation::command_dispatch_task(
        Path::new(&args.dispatch),
        args.task_index,
        args.overwrite,
    )
}

#[cfg(test)]
mod tests {
    use super::parse_agent_environment;

    #[test]
    fn agent_environment_assignments_split_once_and_last_value_wins() {
        let parsed = parse_agent_environment(&[
            "TZ=UTC".to_string(),
            "EMPTY=".to_string(),
            "TOKEN=a=b".to_string(),
            "TZ=America/New_York".to_string(),
        ])
        .unwrap();

        assert_eq!(parsed["TZ"], "America/New_York");
        assert_eq!(parsed["EMPTY"], "");
        assert_eq!(parsed["TOKEN"], "a=b");
    }

    #[test]
    fn agent_environment_assignments_reject_missing_equals_and_unsafe_names() {
        for assignment in ["TZ", "9TZ=UTC", "GIT_DIR=/outside"] {
            let error = parse_agent_environment(&[assignment.to_string()])
                .unwrap_err()
                .to_string();
            assert!(error.contains(assignment), "{assignment}: {error}");
        }
    }
}
