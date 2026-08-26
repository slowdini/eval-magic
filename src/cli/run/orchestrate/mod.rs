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

use crate::adapters::skill_shadow::ShadowSource;
use crate::adapters::{CliDispatchContext, adapter_for};
use crate::cli::command_target_args;
use crate::core::fs::artifact_path;
use crate::core::{
    Assertion, CodebaseRecord, CodebaseSource, CodebaseUse, Eval, GuardPolicyConfig, Mode,
    RunContext, SourceKind, SourceRecord,
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
mod skill;
mod stage;

use skill::{RunSkill, TreatmentSkill};

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
    /// Non-default judge sample count. Absence means one and keeps legacy
    /// manifests byte-compatible.
    pub judge_samples: Option<u32>,
    pub responder_model: Option<&'a str>,
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
    /// Distinct codebases backing the selection, already resolved.
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
    skill_paths_a: Vec<(String, String)>,
    skill_paths_b: Vec<(String, String)>,
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

impl RunCodebase {
    /// The artifact form, shared by every provenance surface so a reader never
    /// has to reconcile two spellings of the same resolution.
    fn record(&self) -> CodebaseRecord {
        CodebaseRecord {
            source: SourceRecord {
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
            },
            exclude_skill_sources: self.declared.exclude_skill_sources(),
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
    /// The codebase backing a private eval environment.
    fn codebase_for(&self, eval_ids: &[String]) -> Result<&RunCodebase, RunError> {
        let [eval_id] = eval_ids else {
            return Err(RunError::msg(format!(
                "private task environment must contain exactly one eval, found: {}",
                eval_ids.join(", ")
            )));
        };
        self.codebases
            .iter()
            .find(|candidate| candidate.eval_ids.contains(eval_id))
            .ok_or_else(|| RunError::msg(format!("eval '{eval_id}' has no resolved codebase")))
    }
}

/// The product of [`stage::stage_conditions`]: the staged slugs plus the
/// dispatch-prompt inputs shared across every task.
struct Staged {
    cond_a_slug: Option<String>,
    cond_b_slug: Option<String>,
    cond_a_skills: Vec<StagedTreatmentSkill>,
    cond_b_skills: Vec<StagedTreatmentSkill>,
    /// Sibling skills' `(name, description)` — env-independent. `build` resolves
    /// the on-disk path for each private task environment.
    sibling_meta: Vec<(String, String)>,
    bootstrap_content: Option<String>,
    plan_mode_content: Option<String>,
    guard_policies: std::collections::HashMap<PathBuf, GuardPolicyConfig>,
    /// Matching project skill sources inventoried from each sourced codebase
    /// before exclusion or staging changes its discovery roots.
    codebase_shadow_sources: std::collections::HashMap<PathBuf, Vec<ShadowSource>>,
}

#[derive(Clone)]
struct StagedTreatmentSkill {
    name: String,
    slug: Option<String>,
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

    // Both ways of driving a conversation need the same capability: without one
    // preserved session, a follow-up answers a fresh agent that never asked.
    // Reported here rather than at dispatch time, so the gap surfaces before a
    // workspace is built.
    if !adapter_for(ctx.harness).has_conversation_resume() {
        let label = adapter_for(ctx.harness).label();
        if resolved
            .selected_evals
            .iter()
            .any(|eval| eval.turns.as_ref().is_some_and(|turns| !turns.is_empty()))
        {
            return Err(RunError::msg(format!(
                "--harness {label} cannot run evals with scripted follow-up turns: its descriptor \
                 declares no [conversation] native resume capability"
            )));
        }
        if resolved
            .selected_evals
            .iter()
            .any(|eval| eval.responder.is_some())
        {
            return Err(RunError::msg(format!(
                "--harness {label} cannot run evals with a responder: its descriptor declares no \
                 [conversation] native resume capability"
            )));
        }
    }

    // The harness preflight enforces the runner-ready dispatch/transcript
    // contract, provides supported enhancements automatically (the write guard
    // auto-arms), and adjusts optional capabilities such as native staging.
    let preflight = super::util::harness_run_preflight(opts, ctx)?;
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
    let render_paths = |paths: &[(String, String)]| {
        if paths.is_empty() {
            "(no skill)".to_string()
        } else if !r.skill.multi {
            paths[0].1.clone()
        } else {
            paths
                .iter()
                .map(|(name, path)| format!("{name}: {path}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    println!("  {}: {}", r.cond_a, render_paths(&r.skill_paths_a));
    println!("  {}: {}", r.cond_b, render_paths(&r.skill_paths_b));
    // The conditions above name the copy; this names where the copy came from,
    // which is what a reader of the report has to be able to find again.
    for treatment in &r.skill.treatments {
        let source = &treatment.source;
        let revision = match (source.revision.as_deref(), source.dirty) {
            (Some(sha), true) => format!(" ({}, uncommitted changes)", &sha[..7.min(sha.len())]),
            (Some(sha), false) => format!(" ({})", &sha[..7.min(sha.len())]),
            (None, _) => String::new(),
        };
        println!(
            "  skill source{}: {}{revision}",
            if !r.skill.multi {
                String::new()
            } else {
                format!(" ({})", treatment.name)
            },
            source.resolved_path.as_deref().unwrap_or(&source.source)
        );
    }
    // The codebases the environments are built from, in the same shape as the
    // skill source line — and the one-checkout-per-iteration fact the caching
    // makes true.
    for codebase in &r.codebases {
        let source = &codebase.source;
        let revision = source
            .revision
            .as_deref()
            .map(|sha| format!(" ({})", &sha[..7.min(sha.len())]))
            .unwrap_or_default();
        println!(
            "  codebase: {}{revision} — materialized once per iteration",
            source.resolved_path.as_deref().unwrap_or(&source.source)
        );
    }
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
    let mut binary_run_counts = BTreeSet::new();
    let mut sampled_endpoints: BTreeSet<(u32, Vec<u32>)> = BTreeSet::new();
    for eval in &r.selected_evals {
        let runs = eval.runs.unwrap_or(opts.runs);
        let sample_counts: BTreeSet<u32> = eval
            .assertions
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|assertion| match assertion {
                Assertion::LlmJudge(judge) => {
                    Some(judge.samples.or(opts.judge_samples).unwrap_or(1))
                }
                _ => None,
            })
            .collect();
        if sample_counts.iter().any(|count| *count > 1) {
            sampled_endpoints.insert((runs, sample_counts.into_iter().collect()));
        } else {
            binary_run_counts.insert(runs);
        }
    }
    for runs in binary_run_counts {
        let run_label = if runs == 1 { "run" } else { "runs" };
        println!(
            "  statistical floor: 2 conditions × {runs} {run_label}; minimum attainable \
             two-sided Fisher exact p on a binary endpoint is {}",
            format_minimum_attainable_fisher_p_value(runs)
        );
    }
    for (runs, sample_counts) in sampled_endpoints {
        let run_label = if runs == 1 { "run" } else { "runs" };
        let counts = sample_counts
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "  statistical endpoint: 2 conditions × {runs} {run_label}; LLM judge sample counts \
             per assertion: {counts}; report vote proportion and pass^k; the binary Fisher exact \
             floor does not apply"
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
    // One command whatever the plan holds: scripted and one-shot tasks are both
    // runner-driven.
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
