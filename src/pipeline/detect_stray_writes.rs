//! Stage 3 — `detect-stray-writes`.
//!
//! Classifies a run's tool
//! invocations against its allowed outputs dir:
//!
//! - **violations**: file-write tools (Write/Edit/MultiEdit/NotebookEdit/Codex
//!   `file_change`) whose target path resolves outside the outputs dir.
//! - **warnings**: shell commands matching a mutating pattern that don't
//!   reference the outputs dir (via the sandbox `classify_bash` policy).
//! - **live_source_reads**: read tools / shell commands that touched the live
//!   skill-under-test directory instead of its staged copy.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::{ConditionsRecord, RunRecord, ToolInvocation};
use crate::pipeline::error::PipelineError;
use crate::pipeline::io::{now_iso8601, write_json};
use crate::pipeline::slots::{run_key, run_slots};
use crate::sandbox::{WRITE_TOOLS, classify_bash, is_under, path_arg};
use crate::validation::{SchemaName, validate_against_schema};

/// Shell-execution tools across harnesses.
const SHELL_TOOLS: [&str; 2] = ["Bash", "command_execution"];
/// Read-only tools that carry a target path argument.
const READ_TOOLS: [&str; 3] = ["Read", "Glob", "Grep"];

/// A file-write tool: a sandbox write tool, or Codex's `file_change`.
fn is_file_write_tool(name: &str) -> bool {
    WRITE_TOOLS.contains(&name) || name == "file_change"
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

/// Classify a run's tool invocations against its allowed outputs dir. See the
/// module docs for what counts as a violation vs. a warning.
pub fn detect_stray_writes(
    invocations: &[ToolInvocation],
    outputs_dir: &str,
    repo_root: &Path,
) -> RunFindings {
    let mut findings = RunFindings::default();

    for inv in invocations {
        if is_file_write_tool(&inv.name) {
            if let Some(p) = inv.args.as_ref().and_then(path_arg)
                && !is_under(p, outputs_dir, repo_root)
            {
                findings.violations.push(StrayFinding {
                    tool: inv.name.clone(),
                    path: Some(p.to_string()),
                    command: None,
                    ordinal: inv.ordinal,
                    reason: "writes outside the run's outputs dir".to_string(),
                });
            }
            continue;
        }

        if SHELL_TOOLS.contains(&inv.name.as_str()) {
            let command = command_of(inv);
            if let Some(reason) =
                classify_bash(command, std::slice::from_ref(&outputs_dir.to_string()))
            {
                findings.warnings.push(StrayFinding {
                    tool: inv.name.clone(),
                    path: None,
                    command: Some(command.to_string()),
                    ordinal: inv.ordinal,
                    reason: reason.to_string(),
                });
            }
        }
    }

    findings
}

/// Lexically absolutize a path (no disk access). Mirrors node's `resolve()`.
fn absolutize(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Node-style lexical `path.relative(from, to)` over absolute, normalized paths.
/// Returns forward-slash-joined components; starts with `..` when `to` is not
/// under `from`.
fn path_relative(from: &Path, to: &Path) -> String {
    let from_comps: Vec<_> = from.components().collect();
    let to_comps: Vec<_> = to.components().collect();
    let mut i = 0;
    while i < from_comps.len() && i < to_comps.len() && from_comps[i] == to_comps[i] {
        i += 1;
    }
    let mut parts: Vec<String> = vec!["..".to_string(); from_comps.len() - i];
    for c in &to_comps[i..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    parts.join("/")
}

/// Leading boundary before a bare `rel` reference: start-of-string or one of
/// `\s'"=:(/`.
fn is_leading_boundary(b: u8) -> bool {
    b.is_ascii_whitespace() || matches!(b, b'\'' | b'"' | b'=' | b':' | b'(' | b'/')
}

/// Trailing boundary after a bare `rel` reference: end-of-string or one of
/// `/\s'")`.
fn is_trailing_boundary(b: u8) -> bool {
    b == b'/' || b.is_ascii_whitespace() || matches!(b, b'\'' | b'"' | b')')
}

/// True if `command` references `rel` as a bare path token — bounded as a path
/// segment and **not** prefixed by a `.claude`/`.agents` staging dir. The
/// `regex` crate has no lookbehind, so each occurrence is scanned directly for
/// the boundary + preceding-segment conditions.
fn references_bare_rel(command: &str, rel: &str) -> bool {
    if rel.is_empty() {
        return false;
    }
    let bytes = command.as_bytes();
    let mut search_from = 0;
    while let Some(off) = command[search_from..].find(rel) {
        let start = search_from + off;
        let end = start + rel.len();

        let leading_ok = start == 0 || is_leading_boundary(bytes[start - 1]);
        // The lookbehind sits before the boundary char: the text up to (but not
        // including) that char must not end with a staging-dir prefix.
        let lookbehind_ok = start == 0 || {
            let before = &command[..start - 1];
            !before.ends_with(".claude") && !before.ends_with(".agents")
        };
        let trailing_ok = end == command.len() || is_trailing_boundary(bytes[end]);

        if leading_ok && lookbehind_ok && trailing_ok {
            return true;
        }
        search_from = start + 1;
    }
    false
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
    let live_dir = absolutize(live_skill_dir);
    let live_dir_str = live_dir.to_string_lossy();
    let rel = path_relative(repo_root, &live_dir);
    let rel_usable = !rel.starts_with("..");

    for inv in invocations {
        if READ_TOOLS.contains(&inv.name.as_str()) {
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

        if SHELL_TOOLS.contains(&inv.name.as_str()) {
            let command = command_of(inv);
            if command.contains(live_dir_str.as_ref())
                || (rel_usable && references_bare_rel(command, &rel))
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
}

/// `dispatch.json` fields the report builder reads (outputs-dir override).
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
    outputs_dir: Option<String>,
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

    let outputs_by_key = outputs_dirs_by_key(iteration_dir);

    let mut runs = Vec::new();
    let mut totals = Totals {
        violations: 0,
        warnings: 0,
        live_source_reads: 0,
    };
    let mut invocations_inspected = 0usize;

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

                let outputs_dir = outputs_by_key
                    .get(&run_key(eval_id, cond, slot.run_index))
                    .cloned()
                    .unwrap_or_else(|| slot.dir.join("outputs").to_string_lossy().into_owned());

                invocations_inspected += run.tool_invocations.len();
                let findings = detect_stray_writes(&run.tool_invocations, &outputs_dir, repo_root);
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

/// Map `"<eval_id>:<condition>[:r<k>]"` → the task's `outputs_dir` from
/// `dispatch.json`. Empty when the file is absent or malformed (callers fall
/// back to convention).
fn outputs_dirs_by_key(iteration_dir: &Path) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if let Ok(raw) = std::fs::read_to_string(iteration_dir.join("dispatch.json"))
        && let Ok(env) = serde_json::from_str::<DispatchEnvelope>(&raw)
    {
        for t in env.tasks.unwrap_or_default() {
            if let Some(dir) = t.outputs_dir {
                out.insert(run_key(&t.eval_id, &t.condition, t.run_index), dir);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const OUTPUTS: &str = "/work/iteration-1/eval-x/with_skill/outputs";
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
    fn a_write_inside_outputs_is_clean() {
        let f = detect_stray_writes(
            &[inv(
                "Write",
                json!({"file_path": format!("{OUTPUTS}/answer.md")}),
                0,
            )],
            OUTPUTS,
            repo(),
        );
        assert!(f.violations.is_empty());
        assert!(f.warnings.is_empty());
    }

    #[test]
    fn a_write_outside_outputs_is_a_violation() {
        let f = detect_stray_writes(
            &[inv(
                "Write",
                json!({"file_path": format!("{REPO}/runner/run.ts")}),
                2,
            )],
            OUTPUTS,
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
    fn edit_multiedit_notebookedit_outside_outputs_is_a_violation() {
        let f = detect_stray_writes(
            &[
                inv("Edit", json!({"file_path": "/etc/hosts"}), 0),
                inv("NotebookEdit", json!({"notebook_path": "/tmp/x.ipynb"}), 1),
            ],
            OUTPUTS,
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
            OUTPUTS,
            repo(),
        );
        assert_eq!(f.warnings.len(), 1);
        assert_eq!(f.warnings[0].tool, "Bash");
        assert!(f.warnings[0].reason.to_lowercase().contains("install"));
    }

    #[test]
    fn a_codex_command_execution_install_is_a_warning() {
        let f = detect_stray_writes(
            &[inv(
                "command_execution",
                json!({"command": "npm install left-pad"}),
                0,
            )],
            OUTPUTS,
            repo(),
        );
        assert_eq!(f.warnings.len(), 1);
        assert_eq!(f.warnings[0].tool, "command_execution");
        assert!(f.warnings[0].reason.to_lowercase().contains("install"));
    }

    #[test]
    fn a_codex_file_change_outside_outputs_is_a_violation() {
        let f = detect_stray_writes(
            &[inv(
                "file_change",
                json!({"path": format!("{REPO}/src/app.ts")}),
                4,
            )],
            OUTPUTS,
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
    fn a_mutating_bash_scoped_to_outputs_is_not_flagged() {
        let f = detect_stray_writes(
            &[inv(
                "Bash",
                json!({"command": format!("echo hi > {OUTPUTS}/log.txt")}),
                0,
            )],
            OUTPUTS,
            repo(),
        );
        assert!(f.warnings.is_empty());
    }

    #[test]
    fn git_worktree_add_is_a_warning() {
        let f = detect_stray_writes(
            &[inv(
                "Bash",
                json!({"command": "git worktree add ../wt -b scratch"}),
                0,
            )],
            OUTPUTS,
            repo(),
        );
        assert_eq!(f.warnings.len(), 1);
        assert!(f.warnings[0].reason.to_lowercase().contains("worktree"));
    }

    #[test]
    fn creating_a_path_under_dot_claude_is_a_warning() {
        let f = detect_stray_writes(
            &[inv("Bash", json!({"command": "mkdir -p .claude/foo"}), 0)],
            OUTPUTS,
            repo(),
        );
        assert_eq!(f.warnings.len(), 1);
        assert!(f.warnings[0].reason.to_lowercase().contains(".claude"));
    }

    #[test]
    fn read_only_tools_are_never_flagged() {
        let f = detect_stray_writes(
            &[
                inv("Read", json!({"file_path": "/anywhere"}), 0),
                inv("Grep", json!({"pattern": "x"}), 1),
                inv("Bash", json!({"command": "ls -la /"}), 2),
            ],
            OUTPUTS,
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
    fn a_bash_referencing_the_live_dir_relatively_is_flagged() {
        let f = detect_live_source_reads(
            &[inv(
                "Bash",
                json!({"command": "cat skills/mr-review/SKILL.md"}),
                3,
            )],
            live(),
            repo(),
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].tool, "Bash");
        assert_eq!(
            f[0].command.as_deref(),
            Some("cat skills/mr-review/SKILL.md")
        );
    }

    #[test]
    fn a_codex_command_referencing_the_live_dir_relatively_is_flagged() {
        let f = detect_live_source_reads(
            &[inv(
                "command_execution",
                json!({"command": "cat skills/mr-review/SKILL.md"}),
                3,
            )],
            live(),
            repo(),
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].tool, "command_execution");
        assert_eq!(
            f[0].command.as_deref(),
            Some("cat skills/mr-review/SKILL.md")
        );
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

    #[test]
    fn a_bash_referencing_a_staged_copy_under_dot_claude_skills_is_not_flagged() {
        let f = detect_live_source_reads(
            &[inv(
                "Bash",
                json!({"command": "cat .claude/skills/mr-review/SKILL.md"}),
                0,
            )],
            live(),
            repo(),
        );
        assert!(f.is_empty());
    }

    #[test]
    fn a_bash_referencing_a_staged_copy_under_dot_agents_skills_is_not_flagged() {
        let f = detect_live_source_reads(
            &[inv(
                "Bash",
                json!({"command": "cat .agents/skills/mr-review/SKILL.md"}),
                0,
            )],
            live(),
            repo(),
        );
        assert!(f.is_empty());
    }

    #[test]
    fn unrelated_reads_and_commands_are_not_flagged() {
        let f = detect_live_source_reads(
            &[
                inv("Read", json!({"file_path": format!("{OUTPUTS}/x.md")}), 0),
                inv("Bash", json!({"command": "ls skills-workspace"}), 1),
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
