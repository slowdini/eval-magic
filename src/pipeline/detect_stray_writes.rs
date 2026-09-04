//! Stage 3 — `detect-stray-writes`.
//!
//! Classifies a run's tool
//! invocations against its allowed task environment:
//!
//! - **violations**: file-write tools (per the adapters' cross-harness
//!   vocabulary union) whose target path resolves outside the task's eval root.
//! - **warnings**: recognized development mutations whose invocation cwd or
//!   explicit destination escapes the eval root, output redirect/`tee` targets
//!   that cannot be proven in bounds, and Git operations that escape the local
//!   task repository.
//! - **live_source_reads**: read tools / shell commands that touched the live
//!   skill-under-test directory instead of its staged copy.
//! - **guard denials**: raw per-task JSONL is joined through `dispatch.json`
//!   into the separate schema-gated `guard-denials.json` artifact, even when a
//!   task has no `run.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::adapters::all_tool_vocabulary;
use crate::adapters::descriptor::PlanFileSection;
use crate::core::fs::{normalize_separators, write_json};
use crate::core::{ConditionsRecord, GuardPolicyConfig, RunRecord, ToolInvocation};
use crate::pipeline::error::PipelineError;
use crate::pipeline::guard_denials::collect_guard_denials;
use crate::pipeline::io::now_iso8601;
use crate::pipeline::slots::{run_key, run_slots};
use crate::sandbox::policy::classify_bash_with_policy;
use crate::sandbox::{
    is_shell_tool, is_under_any, is_under_through_links, is_write_tool, lexically_absolute,
    literal_words, path_arg,
};
use crate::validation::{SchemaName, validate_against_schema};

/// A read-only tool carrying a target path argument, in any harness's
/// vocabulary.
fn is_read_tool(name: &str) -> bool {
    all_tool_vocabulary().read_tools.iter().any(|t| t == name)
}

const LIVE_SOURCE_REASON: &str =
    "reads the live skill source instead of its staged copy — the arm may be contaminated";

/// One flagged tool invocation. `path` is set for write/read findings, `command`
/// for shell findings; the unused one is omitted (the schema forbids extras).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrayFinding {
    pub tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub ordinal: u32,
    pub reason: String,
}

/// The stray-write classification for one run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunFindings {
    pub violations: Vec<StrayFinding>,
    pub warnings: Vec<StrayFinding>,
}

/// The `command` arg of a shell invocation, or `""` when absent.
fn command_of(inv: &ToolInvocation) -> &str {
    inv.args
        .as_ref()
        .and_then(|a| a.get("command"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Classify a run's tool invocations against its allowed task environment. See the
/// module docs for what counts as a violation vs. a warning.
pub fn detect_stray_writes(
    invocations: &[ToolInvocation],
    eval_root: &str,
    invocation_cwd: &Path,
) -> RunFindings {
    detect_stray_writes_with_policy(
        invocations,
        eval_root,
        invocation_cwd,
        &GuardPolicyConfig::default(),
    )
}

/// Classify invocations with the command policy frozen for this task.
pub fn detect_stray_writes_with_policy(
    invocations: &[ToolInvocation],
    eval_root: &str,
    invocation_cwd: &Path,
    guard_policy: &GuardPolicyConfig,
) -> RunFindings {
    detect_stray_writes_in(
        invocations,
        std::slice::from_ref(&eval_root.to_string()),
        invocation_cwd,
        guard_policy,
    )
}

/// Classify invocations against every root a task may write under: its eval
/// root first, then any root the harness declares beside it (the plan file a
/// plan-mode session writes). Mirrors the guard's `allowedRoots`, so the audit
/// and the live guard draw the same boundary.
pub fn detect_stray_writes_in(
    invocations: &[ToolInvocation],
    allowed_roots: &[String],
    invocation_cwd: &Path,
    guard_policy: &GuardPolicyConfig,
) -> RunFindings {
    let mut findings = RunFindings::default();

    for inv in invocations {
        if is_write_tool(&inv.name) {
            if let Some(p) = inv.args.as_ref().and_then(path_arg)
                && !is_under_any(p, allowed_roots, invocation_cwd)
            {
                findings.violations.push(StrayFinding {
                    tool: inv.name.clone(),
                    path: Some(p.to_string()),
                    command: None,
                    ordinal: inv.ordinal,
                    reason: "writes outside the run's task environment".to_string(),
                });
            }
            continue;
        }

        if is_shell_tool(&inv.name) {
            let command = command_of(inv);
            if let Some(classification) =
                classify_bash_with_policy(command, allowed_roots, invocation_cwd, guard_policy)
            {
                findings.warnings.push(StrayFinding {
                    tool: inv.name.clone(),
                    path: None,
                    command: Some(command.to_string()),
                    ordinal: inv.ordinal,
                    reason: classification.reason.to_string(),
                });
            }
        }
    }

    findings
}

/// Whether a shell word spells a path rather than a bare name, by carrying a
/// separator in either spelling.
fn names_a_path(word: &str) -> bool {
    word.contains('/') || word.contains('\\')
}

/// Flag tool invocations that read the **live** skill-under-test directory
/// instead of the staged copy. Reads are detected, not blocked, so this surfaces
/// post-hoc as a validity warning. See `detect-stray-writes.ts` for the rationale.
pub fn detect_live_source_reads(
    invocations: &[ToolInvocation],
    live_skill_dir: &Path,
    repo_root: &Path,
) -> Vec<StrayFinding> {
    let mut findings = Vec::new();
    let live_dir = lexically_absolute(live_skill_dir);
    // Normalized for the shell-command comparison below: the live directory is a
    // host path while the command is whatever the agent typed, so on Windows the
    // two spell the same directory differently.
    let live_dir_str = normalize_separators(&live_dir.to_string_lossy());

    for inv in invocations {
        if is_read_tool(&inv.name) {
            if let Some(p) = inv.args.as_ref().and_then(path_arg)
                && is_under_through_links(p, &live_dir_str, repo_root)
            {
                findings.push(StrayFinding {
                    tool: inv.name.clone(),
                    path: Some(p.to_string()),
                    command: None,
                    ordinal: inv.ordinal,
                    reason: LIVE_SOURCE_REASON.to_string(),
                });
            }
            continue;
        }

        if is_shell_tool(&inv.name) {
            let command = command_of(inv);
            // Neither scan alone covers the command. The raw one reaches
            // spellings no path resolution can — the other separator, a
            // directory named inside a word the lexer marks dynamic — and the
            // per-word one reaches the symlinked route to the live directory,
            // which no substring of the resolved spelling matches.
            //
            // Only words that name a path are resolved: every word resolves
            // against the runner's cwd, so testing bare ones would make each
            // `cargo test` a finding whenever that cwd sits inside the live
            // directory.
            let normalized = normalize_separators(command);
            if normalized.contains(&live_dir_str)
                || literal_words(command).iter().any(|word| {
                    names_a_path(word) && is_under_through_links(word, &live_dir_str, repo_root)
                })
            {
                findings.push(StrayFinding {
                    tool: inv.name.clone(),
                    path: None,
                    command: Some(command.to_string()),
                    ordinal: inv.ordinal,
                    reason: LIVE_SOURCE_REASON.to_string(),
                });
            }
        }
    }

    findings
}

// --- CLI report ---

/// Per-(eval, condition, run) findings, emitted only for runs with ≥1 finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunReport {
    pub eval_id: String,
    pub condition: String,
    /// 1-based run index within a multi-run cell; absent for single-run cells.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_index: Option<u32>,
    pub violations: Vec<StrayFinding>,
    pub warnings: Vec<StrayFinding>,
    pub live_source_reads: Vec<StrayFinding>,
}

/// Aggregate counts across all runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub violations: usize,
    pub warnings: usize,
    pub live_source_reads: usize,
}

/// The full `stray-writes.json` report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StrayWritesReport {
    pub generated: String,
    pub iteration: u32,
    pub totals: Totals,
    pub runs: Vec<RunReport>,
    /// How many transcript tool-calls were actually examined across every run.
    /// Zero means nothing was inspected — a clean `totals` is then *unverifiable*,
    /// not a pass. In-memory only; never serialized into `stray-writes.json`.
    #[serde(skip)]
    pub invocations_inspected: usize,
    /// Denials collected from guarded task envs, regardless of whether a
    /// corresponding run.json exists. In-memory only; the records live in the
    /// separate guard-denials.json artifact.
    #[serde(skip)]
    pub guard_denials: usize,
    /// Operator-facing detail about runs the stage could not fully classify
    /// (e.g. no `eval_root` in `dispatch.json`). Named `notices` rather than
    /// `warnings` because in this module "warning" already means a stray-write
    /// finding of warning severity (see [`Totals::warnings`] and
    /// [`RunReport::warnings`]). In-memory only; the CLI prints these so the
    /// stage itself stays silent.
    #[serde(skip)]
    pub notices: Vec<String>,
}

/// `dispatch.json` fields the report builder reads: the task-environment
/// boundaries, and the frozen descriptor's plan-file root when it declares one.
#[derive(Debug, Deserialize)]
struct DispatchEnvelope {
    tasks: Option<Vec<DispatchRef>>,
    #[serde(default)]
    harness_descriptor: Option<FrozenDescriptorRef>,
}

#[derive(Debug, Deserialize)]
struct FrozenDescriptorRef {
    #[serde(default)]
    plan_mode: Option<FrozenPlanModeRef>,
}

#[derive(Debug, Deserialize)]
struct FrozenPlanModeRef {
    #[serde(default)]
    plan_file: Option<PlanFileSection>,
}

#[derive(Debug, Deserialize)]
struct DispatchRef {
    eval_id: String,
    condition: String,
    #[serde(default)]
    run_index: Option<u32>,
    #[serde(default)]
    eval_root: Option<String>,
    #[serde(default)]
    guard_policy: GuardPolicyConfig,
}

struct TaskBoundary {
    eval_root: String,
    /// The eval root, then the declared plan-file root when there is one.
    allowed_roots: Vec<String>,
    guard_policy: GuardPolicyConfig,
}

/// Build, validate, and write `<iteration_dir>/stray-writes.json` for every
/// `run.json` in the iteration. `repo_root` is the runner's cwd (relative paths
/// resolve against it); `live_skill_dir` is the skill-under-test source.
pub fn detect_stray_writes_report(
    iteration_dir: &Path,
    iteration: u32,
    live_skill_dir: &Path,
    repo_root: &Path,
) -> Result<StrayWritesReport, PipelineError> {
    let conditions_path = iteration_dir.join("conditions.json");
    if !conditions_path.exists() {
        return Err(PipelineError::Message(format!(
            "missing: {}",
            conditions_path.display()
        )));
    }
    let conditions: ConditionsRecord =
        serde_json::from_str(&std::fs::read_to_string(&conditions_path)?)?;
    let live_skill_dirs = conditions
        .skill_source
        .as_ref()
        .and_then(|source| source.skills.as_ref())
        .map(|skills| {
            skills
                .iter()
                .map(|skill| {
                    PathBuf::from(
                        skill
                            .source
                            .resolved_path
                            .as_deref()
                            .unwrap_or(&skill.source.source),
                    )
                })
                .collect::<Vec<_>>()
        })
        .filter(|skills| !skills.is_empty())
        .unwrap_or_else(|| vec![live_skill_dir.to_path_buf()]);
    let condition_names: Vec<String> = conditions
        .conditions
        .iter()
        .map(|c| c.name.clone())
        .collect();

    let boundaries_by_key = task_boundaries_by_key(iteration_dir);
    let guard_denials = collect_guard_denials(iteration_dir, iteration, repo_root)?;

    let mut runs = Vec::new();
    let mut totals = Totals {
        violations: 0,
        warnings: 0,
        live_source_reads: 0,
    };
    let mut invocations_inspected = 0usize;
    let mut notices: Vec<String> = Vec::new();

    let mut eval_dirs: Vec<String> = std::fs::read_dir(iteration_dir)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("eval-").then_some(name)
        })
        .collect();
    eval_dirs.sort();

    for dir_name in &eval_dirs {
        let eval_id = dir_name.strip_prefix("eval-").unwrap_or(dir_name);
        for cond in &condition_names {
            let cond_dir = iteration_dir.join(dir_name).join(cond);
            for slot in run_slots(&cond_dir) {
                let run_path = slot.dir.join("run.json");
                if !run_path.exists() {
                    continue;
                }
                let source = run_path.to_string_lossy();
                let run: RunRecord = validate_against_schema(
                    SchemaName::RunRecord,
                    &serde_json::from_str(&std::fs::read_to_string(&run_path)?)?,
                    &source,
                )?;

                let boundary = boundaries_by_key.get(&run_key(eval_id, cond, slot.run_index));

                invocations_inspected += run.tool_invocations.len();
                // `dispatch.json` is the authoritative source of the private task
                // environment boundary. Without it we skip out-of-bounds write
                // classification rather than guess. Live-source-read detection is
                // independent of this boundary and still runs.
                let findings = match boundary {
                    Some(boundary) => detect_stray_writes_in(
                        &run.tool_invocations,
                        &boundary.allowed_roots,
                        Path::new(&boundary.eval_root),
                        &boundary.guard_policy,
                    ),
                    None => {
                        let run_label = slot
                            .run_index
                            .map(|k| format!(" run-{k}"))
                            .unwrap_or_default();
                        notices.push(format!(
                            "{eval_id}/{cond}{run_label}: no eval_root in dispatch.json — \
                             skipping out-of-bounds write classification (boundary unknown)"
                        ));
                        RunFindings::default()
                    }
                };
                let mut live_reads = Vec::new();
                for live_skill_dir in &live_skill_dirs {
                    for finding in
                        detect_live_source_reads(&run.tool_invocations, live_skill_dir, repo_root)
                    {
                        if !live_reads.contains(&finding) {
                            live_reads.push(finding);
                        }
                    }
                }

                totals.violations += findings.violations.len();
                totals.warnings += findings.warnings.len();
                totals.live_source_reads += live_reads.len();

                if !findings.violations.is_empty()
                    || !findings.warnings.is_empty()
                    || !live_reads.is_empty()
                {
                    runs.push(RunReport {
                        eval_id: eval_id.to_string(),
                        condition: cond.clone(),
                        run_index: slot.run_index,
                        violations: findings.violations,
                        warnings: findings.warnings,
                        live_source_reads: live_reads,
                    });
                }
            }
        }
    }

    let report = StrayWritesReport {
        generated: now_iso8601(),
        iteration,
        totals,
        runs,
        invocations_inspected,
        guard_denials: guard_denials.total_denials,
        notices,
    };

    let out_path = iteration_dir.join("stray-writes.json");
    validate_against_schema::<serde_json::Value>(
        SchemaName::StrayWrites,
        &serde_json::to_value(&report)?,
        &out_path.to_string_lossy(),
    )?;
    write_json(&out_path, &report)?;

    Ok(report)
}

/// Map `"<eval_id>:<condition>[:r<k>]"` to the task boundary and frozen guard
/// policy from `dispatch.json`. Empty when the file is absent or malformed.
fn task_boundaries_by_key(iteration_dir: &Path) -> std::collections::HashMap<String, TaskBoundary> {
    task_boundaries_by_key_with_home(iteration_dir, std::env::home_dir().as_deref())
}

/// [`task_boundaries_by_key`] with the home directory the frozen descriptor's
/// plan-file root (`~/…`) expands against. Without one the plan-file root is
/// left out, and a plan-file write is reported like any other.
fn task_boundaries_by_key_with_home(
    iteration_dir: &Path,
    home: Option<&Path>,
) -> std::collections::HashMap<String, TaskBoundary> {
    let mut out = std::collections::HashMap::new();
    if let Ok(raw) = std::fs::read_to_string(iteration_dir.join("dispatch.json"))
        && let Ok(env) = serde_json::from_str::<DispatchEnvelope>(&raw)
    {
        let plan_file_root = env
            .harness_descriptor
            .and_then(|descriptor| descriptor.plan_mode)
            .and_then(|plan_mode| plan_mode.plan_file)
            .zip(home)
            .map(|(plan_file, home)| plan_file.expanded_root(home).to_string_lossy().into_owned());
        for t in env.tasks.unwrap_or_default() {
            if let Some(eval_root) = t.eval_root {
                let allowed_roots = std::iter::once(eval_root.clone())
                    .chain(plan_file_root.clone())
                    .collect();
                out.insert(
                    run_key(&t.eval_id, &t.condition, t.run_index),
                    TaskBoundary {
                        eval_root,
                        allowed_roots,
                        guard_policy: t.guard_policy,
                    },
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod realistic_development_tests;

#[cfg(test)]
mod tests;
