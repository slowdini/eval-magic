//! `command_run` — the top-level orchestrator that builds an iteration's
//! workspace: validate the request, stage the skill(s), generate every
//! `(eval, condition)` dispatch task, write `dispatch.json` /
//! `dispatch-manifest.md` / `conditions.json`, optionally arm the write guard,
//! and preflight plugin shadows.
//!
//! [`command_run`] is a thin coordinator over four
//! phases, each in its own submodule: [`resolve`] (validate + resolve),
//! [`stage`] (stage the skills), [`build`] (`write_dispatch` + `post_build`),
//! and the two print steps below. The staging and dispatch mechanics live in the
//! sibling [`super::staging`] / [`super::dispatch`] modules, and the small
//! stateless helpers in [`super::util`].

use std::path::PathBuf;

use crate::adapters::{CliDispatchContext, adapter_for};
use crate::cli::command_target_args;
use crate::core::{AvailableSkill, DispatchMechanism, Eval, Mode, RunContext, mechanism_for};

use super::RunError;
use super::util::{insession_dispatch_next_steps, mode_str};

mod build;
mod resolve;
mod stage;

/// Run options parsed from the `run` subcommand flags (everything beyond the
/// shared skill/workspace/harness context, which lives in [`RunContext`]).
#[derive(Debug, Clone, Default)]
pub struct RunOptions<'a> {
    pub mode: Option<&'a str>,
    pub baseline: Option<&'a str>,
    pub only: Option<&'a [String]>,
    pub skip: Option<&'a [String]>,
    pub iteration: Option<u32>,
    pub dry_run: bool,
    pub no_stage: bool,
    pub guard: bool,
    pub stage_name: Option<&'a str>,
    pub plan_mode: bool,
    /// Runs per condition cell; per-eval `runs` overrides take precedence.
    pub runs: u32,
    /// Operator-declared models + label, persisted into `conditions.json` for
    /// provenance (the runner cannot observe them itself).
    pub agent_model: Option<&'a str>,
    pub judge_model: Option<&'a str>,
    pub label: Option<&'a str>,
}

/// Everything [`resolve::resolve_request`] works out before any filesystem
/// mutation: the comparison mode, the selected evals, the iteration coordinates,
/// and each condition's name + skill path.
struct Resolved {
    mode: Mode,
    baseline: Option<String>,
    skill_md_path: PathBuf,
    iteration: u32,
    iteration_dir: PathBuf,
    run_nonce: String,
    run_tag: String,
    cond_a: &'static str,
    cond_b: &'static str,
    skill_path_a: Option<String>,
    skill_path_b: Option<String>,
    selected_evals: Vec<Eval>,
    total_evals: usize,
}

/// The product of [`stage::stage_conditions`]: the staged slugs plus the
/// dispatch-prompt inputs shared across every task.
struct Staged {
    cond_a_slug: Option<String>,
    cond_b_slug: Option<String>,
    sibling_skills: Vec<AvailableSkill>,
    bootstrap_content: Option<String>,
    plan_mode_content: Option<String>,
    /// Whether the harness skills dir existed when `run` started — i.e. before this run staged
    /// anything. Drives the Claude Code staged-skill discovery warning: an existing dir is already
    /// watched, so live change detection surfaces the staged skills; a dir this run had to create
    /// isn't watched until the session re-scans. See [`super::util::staging_discovery_warning`].
    skills_dir_preexisted: bool,
}

/// Build the iteration workspace and dispatch plan for a run.
pub fn command_run(ctx: &RunContext, opts: &RunOptions) -> Result<(), RunError> {
    let resolved = resolve::resolve_request(ctx, opts)?;
    print_run_plan(ctx, opts, &resolved);
    let staged = stage::stage_conditions(ctx, opts, &resolved)?;
    let num_tasks = build::write_dispatch(ctx, opts, &resolved, &staged)?;
    build::post_build(ctx, opts, &resolved, &staged)?;
    print_next_steps(ctx, opts, &resolved, num_tasks);
    Ok(())
}

/// Print the run plan (conditions, selection, staging mode) to stdout.
fn print_run_plan(ctx: &RunContext, opts: &RunOptions, r: &Resolved) {
    println!(
        "Preparing {} iteration-{} ({})",
        ctx.skill_name,
        r.iteration,
        mode_str(r.mode)
    );
    println!(
        "  {}: {}",
        r.cond_a,
        r.skill_path_a.as_deref().unwrap_or("(no skill)")
    );
    println!(
        "  {}: {}",
        r.cond_b,
        r.skill_path_b.as_deref().unwrap_or("(no skill)")
    );
    if r.selected_evals.len() != r.total_evals {
        let (flag, ids) = match (opts.only, opts.skip) {
            (Some(ids), _) => ("--only", ids),
            (_, skip) => ("--skip", skip.unwrap_or(&[])),
        };
        println!(
            "  selection: {} of {} evals ({flag} {})",
            r.selected_evals.len(),
            r.total_evals,
            ids.join(", ")
        );
    }
    if opts.no_stage {
        println!(
            "  staging: disabled (--no-stage) — skills will be inlined into dispatch_prompt for harnesses without project-local skill discovery"
        );
    }
}

/// Print the workspace paths, dispatch count, and the harness-specific next-step
/// instructions.
fn print_next_steps(ctx: &RunContext, opts: &RunOptions, r: &Resolved, num_tasks: usize) {
    let iteration = r.iteration;
    println!("\nWorkspace prepared: {}", r.iteration_dir.display());
    println!(
        "Dispatch manifest:  {}",
        r.iteration_dir.join("dispatch-manifest.md").display()
    );
    println!(
        "Dispatch tasks:     {}",
        r.iteration_dir.join("dispatch.json").display()
    );
    let runbook_path = r.iteration_dir.join("RUNBOOK.md");
    match mechanism_for(ctx.harness) {
        DispatchMechanism::InSession => println!(
            "Runbook:            {} — for an isolated run, start a fresh session and say \"Read and follow RUNBOOK.md\".",
            runbook_path.display()
        ),
        DispatchMechanism::Cli => println!(
            "Runbook:            {} — a human-followed copy of the steps below.",
            runbook_path.display()
        ),
    }
    let run_counts: Vec<u32> = r
        .selected_evals
        .iter()
        .map(|e| e.runs.unwrap_or(opts.runs))
        .collect();
    let uniform_runs = run_counts
        .first()
        .filter(|&&n| run_counts.iter().all(|&m| m == n));
    match uniform_runs {
        Some(1) => println!(
            "\n{} dispatches required ({} evals × 2 conditions).",
            num_tasks,
            r.selected_evals.len()
        ),
        Some(n) => println!(
            "\n{} dispatches required ({} evals × 2 conditions × {n} runs).",
            num_tasks,
            r.selected_evals.len()
        ),
        None => println!(
            "\n{} dispatches required ({} evals × 2 conditions, per-eval run counts).",
            num_tasks,
            r.selected_evals.len()
        ),
    }

    if opts.dry_run {
        println!("\n--dry-run: stopping after workspace prep.");
        return;
    }
    let target_args = command_target_args(ctx);
    match mechanism_for(ctx.harness) {
        // In-session subagent dispatch (Claude Code's Task tool today). The
        // dispatch-loop guidance is shared with the interactive runbook
        // ([`super::runbook`]) so the two can never drift.
        DispatchMechanism::InSession => println!(
            "\nNext: {}",
            insession_dispatch_next_steps(&target_args, iteration)
        ),
        // One-shot CLI dispatch; the exact command is harness-specific.
        DispatchMechanism::Cli => println!(
            "{}",
            adapter_for(ctx.harness).cli_next_steps(CliDispatchContext {
                guard: opts.guard,
                target_args: &target_args,
                iteration,
                agent_model: opts.agent_model,
            })
        ),
    }
}
