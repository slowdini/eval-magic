//! `command_run` — the top-level orchestrator that builds an iteration's
//! workspace: validate the request, stage the skill(s), generate every
//! `(eval, condition, run)` dispatch task, write `dispatch.json` /
//! `dispatch-manifest.md` / `conditions.json`, optionally arm the write guard,
//! and preflight plugin shadows.
//!
//! [`command_run`] is a thin coordinator over four
//! phases, each in its own submodule: [`resolve`] (validate + resolve),
//! [`stage`] (stage the skills), [`build`] (`write_dispatch` + `post_build`),
//! and the two print steps below. The staging and dispatch mechanics live in the
//! sibling [`super::staging`] / [`super::dispatch`] modules, and the small
//! stateless helpers in [`super::util`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::adapters::{CliDispatchContext, adapter_for};
use crate::cli::command_target_args;
use crate::core::fs::artifact_path;
use crate::core::{
    CodebaseSource, CodebaseUse, Eval, Mode, RunContext, SkillSource, SourceKind, SourceRecord,
};
use crate::source::ResolvedSource;

use super::RunError;
use super::statistics::format_minimum_attainable_fisher_p_value;
use super::util::mode_str;

mod build;
mod envs;
mod git;
mod resolve;
mod shadow_preflight;
mod shell;
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
    /// Tri-state write guard: `None` = auto (the preflight resolves it to
    /// `Some` — armed when the harness declares a guard and staging is
    /// active), `Some` = explicit `--guard` / `--no-guard`.
    pub guard: Option<bool>,
    pub stage_name: Option<&'a str>,
    pub plan_mode: bool,
    /// Runs per condition cell; per-eval `runs` overrides take precedence.
    pub runs: u32,
    /// Operator-declared models + label, persisted into `conditions.json` for
    /// provenance (the runner cannot observe them itself).
    pub agent_model: Option<&'a str>,
    /// Resolved descriptor defaults plus run-level agent environment overrides.
    pub agent_env: BTreeMap<String, String>,
    pub judge_model: Option<&'a str>,
    pub label: Option<&'a str>,
}

impl RunOptions<'_> {
    /// Whether the preflight-resolved guard is armed. (`None` only occurs
    /// before the preflight runs; it reads as unarmed.)
    pub(crate) fn guard_armed(&self) -> bool {
        self.guard == Some(true)
    }
}

/// Everything [`resolve::resolve_request`] works out before any filesystem
/// mutation: the comparison mode, the selected evals, the iteration coordinates,
/// and each condition's name + skill path.
struct Resolved {
    mode: Mode,
    baseline: Option<String>,
    /// Distinct codebases the selection declares, already resolved to a commit.
    /// Empty for a fixture-only run, which is what keeps that path unchanged.
    codebases: Vec<RunCodebase>,
    /// The skill under test, resolved but not yet copied.
    skill: RunSkill,
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
    /// Task-scoped groups computed from the selected evals in config order.
    /// Always at least one group (`g1`) for a non-empty selection.
    groups: Vec<super::grouping::Group>,
}

/// One resolved codebase and the evals built from it.
struct RunCodebase {
    /// The declaration as written, which is what deduplication compares.
    declared: CodebaseSource,
    source: ResolvedSource,
    /// Directory name under `iteration-N/.codebase/` this materializes into.
    key: String,
    eval_ids: Vec<String>,
}

/// Where an iteration keeps its copies of the skills under test.
///
/// A sibling of `.codebase/`, and scaffolding in the same sense: it holds inputs
/// the runner placed, above every environment root, so an agent reaching it is
/// already a stray write.
pub(super) fn skills_copy_root(iteration_dir: &Path) -> PathBuf {
    iteration_dir.join(".skills")
}

/// The resolved skill under test and the sibling roster staged with it.
struct RunSkill {
    source: ResolvedSource,
    /// Captured at resolution. Staging copies exactly these names, so what the
    /// record claims and what the environments hold cannot drift apart.
    siblings: Vec<String>,
}

impl RunSkill {
    fn record(&self) -> SkillSource {
        SkillSource {
            source: SourceRecord {
                // A skill is named by a path on this host; there is no url form.
                kind: SourceKind::Path,
                source: self.source.source.clone(),
                resolved_path: self
                    .source
                    .resolved_path
                    .as_deref()
                    .map(|path| artifact_path(Path::new(path))),
                reference: self.source.reference.clone(),
                revision: self.source.revision.clone(),
                origin_url: self.source.origin_url.clone(),
                branch: self.source.branch.clone(),
                host_local: self.source.host_local,
                // The copy is the working tree as it sits, so an uncommitted edit
                // is in what ran and the revision alone does not name it.
                dirty: self.source.dirty,
            },
            siblings: self.siblings.clone(),
        }
    }
}

impl RunCodebase {
    /// The artifact form, shared by every provenance surface so a reader never
    /// has to reconcile two spellings of the same resolution.
    fn record(&self) -> SourceRecord {
        SourceRecord {
            kind: match self.declared {
                CodebaseSource::Git { .. } => SourceKind::Git,
                CodebaseSource::Path { .. } => SourceKind::Path,
            },
            source: self.source.source.clone(),
            resolved_path: self
                .source
                .resolved_path
                .as_deref()
                .map(|path| artifact_path(Path::new(path))),
            reference: self.source.reference.clone(),
            revision: self.source.revision.clone(),
            origin_url: self.source.origin_url.clone(),
            branch: self.source.branch.clone(),
            host_local: self.source.host_local,
            // Materialization checks out a commit, so the environment never
            // carries uncommitted work however the source directory looked.
            dirty: false,
        }
    }

    fn usage(&self) -> CodebaseUse {
        CodebaseUse {
            codebase: self.record(),
            evals: self.eval_ids.clone(),
        }
    }
}

impl Resolved {
    /// The codebase backing an environment, given the evals sharing it.
    ///
    /// Production always task-scopes, so an environment carries exactly one
    /// eval and the question is trivial. The error covers the planner's older
    /// multi-eval grouping, where two evals with different codebases could not
    /// share one working tree even in principle.
    fn codebase_for(&self, eval_ids: &[String]) -> Result<Option<&RunCodebase>, RunError> {
        let mut found: Option<&RunCodebase> = None;
        for eval_id in eval_ids {
            let codebase = self
                .codebases
                .iter()
                .find(|candidate| candidate.eval_ids.contains(eval_id));
            match (found, codebase) {
                (None, next) => found = next,
                (Some(previous), Some(next)) if !std::ptr::eq(previous, next) => {
                    return Err(RunError::msg(format!(
                        "evals {} share an environment but declare different codebases; \
                         give them distinct environments",
                        eval_ids.join(", ")
                    )));
                }
                _ => {}
            }
        }
        Ok(found)
    }
}

/// The product of [`stage::stage_conditions`]: the staged slugs plus the
/// dispatch-prompt inputs shared across every task.
struct Staged {
    cond_a_slug: Option<String>,
    cond_b_slug: Option<String>,
    /// Sibling skills' `(name, description)` — env-independent. `build` resolves
    /// the on-disk path for each private task environment.
    sibling_meta: Vec<(String, String)>,
    bootstrap_content: Option<String>,
    plan_mode_content: Option<String>,
}

/// Build the iteration workspace and dispatch plan for a run.
pub fn command_run(ctx: &RunContext, opts: &RunOptions) -> Result<(), RunError> {
    // Git is a hard runtime dependency for task-repository isolation. Probe it
    // before resolution chooses or creates any iteration workspace.
    git::preflight_git(ctx)?;

    // The POSIX toolchain is a requirement of the *recipes* this run generates,
    // not of the run itself, so it warns and continues (issue #248). Printed up
    // front: an operator who has to install something should learn it before
    // reading a runbook full of commands their shell cannot parse.
    for warning in shell::preflight_posix_tooling() {
        eprintln!("⚠ {warning}");
    }

    // Resolve first (read-only): the preflight scopes its transcript warning
    // to the eval config actually selected for the run.
    let resolved = resolve::resolve_request(ctx, opts)?;

    if resolved
        .selected_evals
        .iter()
        .any(|eval| eval.turns.as_ref().is_some_and(|turns| !turns.is_empty()))
        && !adapter_for(ctx.harness).has_conversation_resume()
    {
        return Err(RunError::msg(format!(
            "--harness {} cannot run evals with scripted follow-up turns: its descriptor \
             declares no [conversation] native resume capability",
            adapter_for(ctx.harness).label()
        )));
    }

    // The harness preflight provides supported enhancements automatically (the
    // write guard auto-arms), warns about undeclared ones (naming each
    // fallback), and adjusts the options — it only rejects genuinely
    // contradictory flag combinations.
    let preflight = super::util::harness_run_preflight(
        opts,
        ctx,
        super::util::evals_use_transcript_check(&resolved.selected_evals),
    )?;
    for warning in &preflight.warnings {
        eprintln!("⚠ {warning}");
    }
    let opts = &preflight.opts;

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
    // The conditions above name the copy; this names where the copy came from,
    // which is what a reader of the report has to be able to find again.
    let source = &r.skill.source;
    let revision = match (source.revision.as_deref(), source.dirty) {
        (Some(sha), true) => format!(" ({}, uncommitted changes)", &sha[..7.min(sha.len())]),
        (Some(sha), false) => format!(" ({})", &sha[..7.min(sha.len())]),
        (None, _) => String::new(),
    };
    println!(
        "  skill source: {}{revision}",
        source.resolved_path.as_deref().unwrap_or(&source.source)
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
    let effective_run_counts: BTreeSet<u32> = r
        .selected_evals
        .iter()
        .map(|eval| eval.runs.unwrap_or(opts.runs))
        .collect();
    for runs in effective_run_counts {
        let run_label = if runs == 1 { "run" } else { "runs" };
        println!(
            "  statistical floor: 2 conditions × {runs} {run_label}; minimum attainable \
             two-sided Fisher exact p on a binary endpoint is {}",
            format_minimum_attainable_fisher_p_value(runs)
        );
    }
    if opts.no_stage {
        println!(
            "  staging: disabled (--no-stage) — skills will be inlined into dispatch_prompt for harnesses without project-local skill discovery"
        );
    }
    if opts.guard_armed() {
        println!(
            "  guard: armed — the write guard blocks out-of-env writes during dispatch (--no-guard to opt out)"
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

    println!(
        "Runbook:            {} — a human-followed copy of the steps below.",
        r.iteration_dir.join("RUNBOOK.md").display()
    );
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
    if r.selected_evals.iter().any(|eval| eval.turns.is_some()) {
        let mix = r.selected_evals.iter().any(|eval| eval.turns.is_none());
        println!(
            "\nNext: read RUNBOOK.md and run every task with scripted `turns` through \
             `eval-magic dispatch-task` so follow-ups resume the same native session.{} \
             Then run `eval-magic ingest{target_args} --iteration {} --harness {}`.",
            if mix {
                " Use its harness recipe only for the remaining one-shot tasks."
            } else {
                ""
            },
            r.iteration,
            adapter_for(ctx.harness).label()
        );
        return;
    }
    // One-shot CLI dispatch; the exact command is harness-specific.
    println!(
        "{}",
        adapter_for(ctx.harness).cli_next_steps(CliDispatchContext {
            guard: opts.guard_armed(),
            target_args: &target_args,
            iteration,
            agent_model: opts.agent_model,
            agent_env: &opts.agent_env,
        })
    );
}
