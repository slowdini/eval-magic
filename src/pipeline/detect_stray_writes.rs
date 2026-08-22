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

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::adapters::all_tool_vocabulary;
use crate::core::fs::{normalize_separators, write_json};
use crate::core::{ConditionsRecord, GuardPolicyConfig, RunRecord, ToolInvocation};
use crate::pipeline::error::PipelineError;
use crate::pipeline::guard_denials::collect_guard_denials;
use crate::pipeline::io::now_iso8601;
use crate::pipeline::slots::{run_key, run_slots};
use crate::sandbox::policy::classify_bash_with_policy;
use crate::sandbox::{is_shell_tool, is_under, is_write_tool, lexically_absolute, path_arg};
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
    let mut findings = RunFindings::default();

    for inv in invocations {
        if is_write_tool(&inv.name) {
            if let Some(p) = inv.args.as_ref().and_then(path_arg)
                && !is_under(p, eval_root, invocation_cwd)
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
            if let Some(classification) = classify_bash_with_policy(
                command,
                std::slice::from_ref(&eval_root.to_string()),
                invocation_cwd,
                guard_policy,
            ) {
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
                && is_under(p, &live_dir_str, repo_root)
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
            let normalized = normalize_separators(command);
            if normalized.contains(&live_dir_str) {
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

/// `dispatch.json` fields the report builder reads (task-environment boundary).
#[derive(Debug, Deserialize)]
struct DispatchEnvelope {
    tasks: Option<Vec<DispatchRef>>,
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
                    Some(boundary) => detect_stray_writes_with_policy(
                        &run.tool_invocations,
                        &boundary.eval_root,
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
                let live_reads =
                    detect_live_source_reads(&run.tool_invocations, live_skill_dir, repo_root);

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
    let mut out = std::collections::HashMap::new();
    if let Ok(raw) = std::fs::read_to_string(iteration_dir.join("dispatch.json"))
        && let Ok(env) = serde_json::from_str::<DispatchEnvelope>(&raw)
    {
        for t in env.tasks.unwrap_or_default() {
            if let Some(eval_root) = t.eval_root {
                out.insert(
                    run_key(&t.eval_id, &t.condition, t.run_index),
                    TaskBoundary {
                        eval_root,
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
mod tests {
    use super::*;
    use serde_json::json;

    const ALLOWED_ROOT: &str = "/work/iteration-1/env-g1-with_skill";
    const REPO: &str = "/work/repo";
    const LIVE_SKILL: &str = "/work/repo/skills/mr-review";

    /// Build a minimal invocation from name/args/ordinal (result is unused here).
    fn inv(name: &str, args: serde_json::Value, ordinal: u32) -> ToolInvocation {
        ToolInvocation {
            name: name.to_string(),
            args: Some(args),
            result: None,
            ordinal,
        }
    }

    fn repo() -> &'static Path {
        Path::new(REPO)
    }

    fn live() -> &'static Path {
        Path::new(LIVE_SKILL)
    }

    // --- detectStrayWrites ---

    #[test]
    fn a_write_inside_the_task_environment_is_clean() {
        let f = detect_stray_writes(
            &[inv(
                "Write",
                json!({"file_path": format!("{ALLOWED_ROOT}/answer.md")}),
                0,
            )],
            ALLOWED_ROOT,
            repo(),
        );
        assert!(f.violations.is_empty());
        assert!(f.warnings.is_empty());
    }

    #[test]
    fn a_relative_write_resolves_from_the_task_environment() {
        let f = detect_stray_writes(
            &[inv("Edit", json!({"file_path": "src/lib.rs"}), 0)],
            ALLOWED_ROOT,
            Path::new(ALLOWED_ROOT),
        );
        assert!(f.violations.is_empty());
    }

    #[test]
    fn a_write_outside_the_task_environment_is_a_violation() {
        let f = detect_stray_writes(
            &[inv(
                "Write",
                json!({"file_path": format!("{REPO}/runner/run.ts")}),
                2,
            )],
            ALLOWED_ROOT,
            repo(),
        );
        assert_eq!(f.violations.len(), 1);
        assert_eq!(f.violations[0].tool, "Write");
        assert_eq!(
            f.violations[0].path.as_deref(),
            Some(&*format!("{REPO}/runner/run.ts"))
        );
        assert_eq!(f.violations[0].ordinal, 2);
    }

    #[test]
    fn edit_multiedit_notebookedit_outside_the_task_environment_is_a_violation() {
        let f = detect_stray_writes(
            &[
                inv("Edit", json!({"file_path": "/etc/hosts"}), 0),
                inv("NotebookEdit", json!({"notebook_path": "/tmp/x.ipynb"}), 1),
            ],
            ALLOWED_ROOT,
            repo(),
        );
        let mut tools: Vec<&str> = f.violations.iter().map(|v| v.tool.as_str()).collect();
        tools.sort();
        assert_eq!(tools, vec!["Edit", "NotebookEdit"]);
    }

    #[test]
    fn an_install_command_is_a_warning() {
        let f = detect_stray_writes(
            &[inv("Bash", json!({"command": "npm install left-pad"}), 0)],
            ALLOWED_ROOT,
            repo(),
        );
        assert_eq!(f.warnings.len(), 1);
        assert_eq!(f.warnings[0].tool, "Bash");
        assert!(f.warnings[0].reason.to_lowercase().contains("install"));
    }

    #[test]
    fn configured_command_policy_is_shared_with_the_stray_write_audit() {
        let policy = crate::core::GuardPolicyConfig {
            allow_commands: vec!["cargo test".to_string()],
            ..crate::core::GuardPolicyConfig::default()
        };

        let findings = detect_stray_writes_with_policy(
            &[inv("Bash", json!({"command": "cargo test --workspace"}), 0)],
            ALLOWED_ROOT,
            Path::new(ALLOWED_ROOT),
            &policy,
        );

        assert!(findings.warnings.is_empty());
    }

    #[test]
    fn a_codex_command_execution_install_is_a_warning() {
        let f = detect_stray_writes(
            &[inv(
                "command_execution",
                json!({"command": "npm install left-pad"}),
                0,
            )],
            ALLOWED_ROOT,
            repo(),
        );
        assert_eq!(f.warnings.len(), 1);
        assert_eq!(f.warnings[0].tool, "command_execution");
        assert!(f.warnings[0].reason.to_lowercase().contains("install"));
    }

    #[test]
    fn a_codex_file_change_outside_the_task_environment_is_a_violation() {
        let f = detect_stray_writes(
            &[inv(
                "file_change",
                json!({"path": format!("{REPO}/src/app.ts")}),
                4,
            )],
            ALLOWED_ROOT,
            repo(),
        );
        assert_eq!(f.violations.len(), 1);
        assert_eq!(f.violations[0].tool, "file_change");
        assert_eq!(
            f.violations[0].path.as_deref(),
            Some(&*format!("{REPO}/src/app.ts"))
        );
        assert_eq!(f.violations[0].ordinal, 4);
    }

    #[test]
    fn a_mutating_bash_scoped_to_the_task_environment_is_not_flagged() {
        let f = detect_stray_writes(
            &[inv(
                "Bash",
                json!({"command": format!("echo hi > {ALLOWED_ROOT}/log.txt")}),
                0,
            )],
            ALLOWED_ROOT,
            repo(),
        );
        assert!(f.warnings.is_empty());
    }

    #[test]
    fn a_relative_redirection_resolves_from_the_task_environment() {
        let f = detect_stray_writes(
            &[inv(
                "Bash",
                json!({"command": "printf done > final-message.md"}),
                0,
            )],
            ALLOWED_ROOT,
            Path::new(ALLOWED_ROOT),
        );
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    }

    #[test]
    fn git_worktree_add_is_a_warning() {
        let f = detect_stray_writes(
            &[inv(
                "Bash",
                json!({"command": "git worktree add ../wt -b scratch"}),
                0,
            )],
            ALLOWED_ROOT,
            repo(),
        );
        assert_eq!(f.warnings.len(), 1);
        assert!(f.warnings[0].reason.to_lowercase().contains("worktree"));
    }

    #[test]
    fn read_only_tools_are_never_flagged() {
        let f = detect_stray_writes(
            &[
                inv("Read", json!({"file_path": "/anywhere"}), 0),
                inv("Grep", json!({"pattern": "x"}), 1),
                inv("Bash", json!({"command": "ls -la /"}), 2),
            ],
            ALLOWED_ROOT,
            repo(),
        );
        assert!(f.violations.is_empty());
        assert!(f.warnings.is_empty());
    }

    // --- detectLiveSourceReads ---

    #[test]
    fn a_read_of_the_live_skill_md_is_flagged() {
        let f = detect_live_source_reads(
            &[inv(
                "Read",
                json!({"file_path": format!("{LIVE_SKILL}/SKILL.md")}),
                1,
            )],
            live(),
            repo(),
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].tool, "Read");
        assert_eq!(
            f[0].path.as_deref(),
            Some(&*format!("{LIVE_SKILL}/SKILL.md"))
        );
        assert_eq!(f[0].ordinal, 1);
        assert!(f[0].reason.to_lowercase().contains("live skill source"));
    }

    #[test]
    fn a_read_of_a_staged_eval_copy_is_not_flagged() {
        let f = detect_live_source_reads(
            &[inv(
                "Read",
                json!({"file_path": format!("{REPO}/.claude/skills/slow-powers-eval-1-old_skill__mr-review/SKILL.md")}),
                0,
            )],
            live(),
            repo(),
        );
        assert!(f.is_empty());
    }

    #[test]
    fn a_relative_read_resolving_under_the_live_dir_is_flagged() {
        let f = detect_live_source_reads(
            &[inv(
                "Read",
                json!({"file_path": "skills/mr-review/SKILL.md"}),
                0,
            )],
            live(),
            repo(),
        );
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn a_grep_scoped_to_the_live_dir_is_flagged() {
        let f = detect_live_source_reads(
            &[inv("Grep", json!({"pattern": "x", "path": LIVE_SKILL}), 2)],
            live(),
            repo(),
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].tool, "Grep");
    }

    #[test]
    fn a_bash_referencing_the_live_dir_absolutely_is_flagged() {
        let f = detect_live_source_reads(
            &[inv(
                "Bash",
                json!({"command": format!("grep -r trigger {LIVE_SKILL}/")}),
                0,
            )],
            live(),
            repo(),
        );
        assert_eq!(f.len(), 1);
    }

    /// A contaminated arm is not comparable data, so the scan has to hold when
    /// the command spells the live directory with the other separator — which
    /// on Windows is every command, since the recorded directory is a host path.
    #[test]
    fn a_bash_spelling_the_live_dir_with_the_other_separator_is_flagged() {
        let f = detect_live_source_reads(
            &[inv(
                "Bash",
                json!({"command": r"cat \work\repo\skills\mr-review\SKILL.md"}),
                0,
            )],
            live(),
            repo(),
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].tool, "Bash");
    }

    #[test]
    fn unrelated_reads_and_commands_are_not_flagged() {
        let f = detect_live_source_reads(
            &[
                inv(
                    "Read",
                    json!({"file_path": format!("{ALLOWED_ROOT}/x.md")}),
                    0,
                ),
                inv("Bash", json!({"command": "ls .eval-magic"}), 1),
                // Write tools are detect_stray_writes' jurisdiction — reads only here.
                inv(
                    "Write",
                    json!({"file_path": format!("{LIVE_SKILL}/SKILL.md")}),
                    2,
                ),
            ],
            live(),
            repo(),
        );
        assert!(f.is_empty());
    }
}
