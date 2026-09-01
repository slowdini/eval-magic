//! Runner-owned command-check grading.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path};
use std::process::{Command, ExitStatus};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::core::fs::{copy_entry_materialized, write_json};
use crate::core::{Assertion, AssertionCommandCheck, clear_git_environment};
use crate::pipeline::error::PipelineError;
use crate::pipeline::grade::instrument::GradingInstrument;
use crate::validation::{SchemaName, validate_against_schema};

const DIAGNOSTIC_LIMIT: usize = 2 * 1024;

/// The schema-gated intermediate result persisted before finalize converts it
/// into a normal [`crate::core::AssertionResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandCheckResult {
    pub id: String,
    pub passed: bool,
    pub evidence: String,
    pub expected_exit_code: i32,
    pub actual_exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cells: Option<Vec<CommandCheckCellResult>>,
    /// Digest of the `command_check` this result came from. Reuse is keyed by
    /// assertion id, so this is what tells a later grade that the check under
    /// that id has been edited since. Absent in results that predate the record,
    /// which make no claim either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_digest: Option<String>,
}

/// The result of one environment-matrix cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandCheckCellResult {
    pub env: BTreeMap<String, String>,
    pub passed: bool,
    pub evidence: String,
    pub actual_exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CommandCheckSummary {
    pub executed: usize,
    pub reused: usize,
    pub failed: usize,
    /// Reused results whose check has since been edited. Returned rather than
    /// printed: the CLI handler owns how a warning reads.
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DispatchFile {
    #[serde(default)]
    tasks: Vec<DispatchTask>,
}

#[derive(Debug, Deserialize)]
struct DispatchTask {
    eval_id: String,
    condition: String,
    eval_root: Option<String>,
    run_record_path: String,
}

/// Inject held-out setup files and execute all command checks in declaration
/// order for every matching dispatch task.
pub fn grade_command_checks(
    iteration_dir: &Path,
    instrument: &GradingInstrument,
    overwrite: bool,
) -> Result<CommandCheckSummary, PipelineError> {
    let evals = &instrument.evals;
    let has_command_checks = evals.evals.iter().any(|eval| {
        eval.assertions
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .any(|assertion| matches!(assertion, Assertion::CommandCheck(_)))
    });
    if !has_command_checks {
        return Ok(CommandCheckSummary::default());
    }

    let dispatch_path = iteration_dir.join("dispatch.json");
    let dispatch: DispatchFile =
        serde_json::from_str(&fs::read_to_string(&dispatch_path).map_err(|error| {
            PipelineError::Message(format!(
                "could not read {} for command_check grading: {error}",
                dispatch_path.display()
            ))
        })?)?;
    let root_counts: HashMap<&str, usize> = dispatch
        .tasks
        .iter()
        .filter_map(|task| task.eval_root.as_deref())
        .fold(HashMap::new(), |mut counts, root| {
            *counts.entry(root).or_default() += 1;
            counts
        });
    let mut summary = CommandCheckSummary::default();

    for task in &dispatch.tasks {
        let Some(eval) = evals.evals.iter().find(|eval| eval.id == task.eval_id) else {
            continue;
        };
        let checks: Vec<&AssertionCommandCheck> = eval
            .assertions
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|assertion| match assertion {
                Assertion::CommandCheck(check) => Some(check),
                _ => None,
            })
            .collect();
        if checks.is_empty() {
            continue;
        }

        let eval_root = task
            .eval_root
            .as_deref()
            .ok_or_else(|| isolation_error(task, "does not record an eval_root"))?;
        if root_counts.get(eval_root).copied().unwrap_or_default() != 1 {
            return Err(isolation_error(
                task,
                "shares eval_root with another dispatch task",
            ));
        }
        let eval_root = Path::new(eval_root);
        let run_dir = Path::new(&task.run_record_path).parent().ok_or_else(|| {
            PipelineError::Message(format!(
                "command_check task '{}'/{} has no run directory in run_record_path",
                task.eval_id, task.condition
            ))
        })?;
        let results_dir = run_dir.join("command-checks");

        for check in checks {
            validate_assertion_id(&check.id)?;
            let digest = definition_digest(check);
            let result_path = results_dir.join(format!("{}.json", check.id));
            if result_path.exists() && !overwrite {
                let value = serde_json::from_str(&fs::read_to_string(&result_path)?)?;
                let reused = validate_against_schema::<CommandCheckResult>(
                    SchemaName::CommandCheck,
                    &value,
                    &result_path.to_string_lossy(),
                )?;
                if reused
                    .definition_digest
                    .is_some_and(|recorded| recorded != digest)
                {
                    summary.warnings.push(format!(
                        "command_check '{}' for {}/{} changed since its cached result was produced; that result is reused as-is. Re-run grade with --overwrite to execute the edited check.",
                        check.id, task.eval_id, task.condition
                    ));
                }
                summary.reused += 1;
                continue;
            }

            inject_setup_files(check, instrument.setup_root_for(&task.eval_id), eval_root)?;
            let result = execute_command_check(check, eval_root)?;
            if !result.passed {
                summary.failed += 1;
            }
            fs::create_dir_all(&results_dir)?;
            validate_against_schema::<CommandCheckResult>(
                SchemaName::CommandCheck,
                &serde_json::to_value(&result)?,
                &result_path.to_string_lossy(),
            )?;
            write_json(&result_path, &result)?;
            summary.executed += 1;
        }
    }

    Ok(summary)
}

/// Digest of a check's authored definition, so reuse can tell an edited check
/// from the one that produced the cached result.
fn definition_digest(check: &AssertionCommandCheck) -> String {
    crate::core::fs::fnv1a_hex(
        serde_json::to_string(check)
            .expect("an authored command_check serializes")
            .as_bytes(),
    )
}

fn isolation_error(task: &DispatchTask, detail: &str) -> PipelineError {
    PipelineError::Message(format!(
        "command_check task '{}'/{} {detail}; command checks require task-scoped environments. Build and dispatch a fresh iteration with this evals.json before grading.",
        task.eval_id, task.condition
    ))
}

fn validate_assertion_id(id: &str) -> Result<(), PipelineError> {
    let components: Vec<_> = Path::new(id).components().collect();
    if !matches!(components.as_slice(), [Component::Normal(_)]) {
        return Err(PipelineError::Message(format!(
            "command_check assertion id must be one path-safe component: {id}"
        )));
    }
    Ok(())
}

fn inject_setup_files(
    check: &AssertionCommandCheck,
    skill_dir: &Path,
    eval_root: &Path,
) -> Result<(), PipelineError> {
    for relative in check.setup_files.as_deref().unwrap_or(&[]) {
        validate_setup_relative(relative)?;
        let source = skill_dir.join("evals").join(relative);
        if !source.exists() {
            return Err(PipelineError::Message(format!(
                "command-check setup file not found during grading: {}",
                source.display()
            )));
        }
        let destination = eval_root.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        copy_entry_materialized(&source, &destination)?;
    }
    Ok(())
}

fn validate_setup_relative(relative: &str) -> Result<(), PipelineError> {
    let unsafe_component = Path::new(relative).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if unsafe_component {
        return Err(PipelineError::Message(format!(
            "command_check setup path must be relative and stay within the task environment: {relative}"
        )));
    }
    Ok(())
}

/// Execute one trusted command-check assertion through the platform shell.
pub(super) fn execute_command_check(
    assertion: &AssertionCommandCheck,
    eval_root: &Path,
) -> Result<CommandCheckResult, PipelineError> {
    validate_command_environment(assertion)?;

    let Some(matrix) = &assertion.matrix else {
        let env = assertion.env.clone().unwrap_or_default();
        let cell = execute_command_check_cell(assertion, eval_root, env)?;
        return Ok(CommandCheckResult {
            id: assertion.id.clone(),
            passed: cell.passed,
            evidence: cell.evidence,
            expected_exit_code: assertion.expect_exit_code,
            actual_exit_code: cell.actual_exit_code,
            stdout: cell.stdout,
            stderr: cell.stderr,
            cells: None,
            definition_digest: Some(definition_digest(assertion)),
        });
    };

    let cells = matrix_environments(assertion)
        .into_iter()
        .map(|env| execute_command_check_cell(assertion, eval_root, env))
        .collect::<Result<Vec<_>, _>>()?;
    let passed_count = cells.iter().filter(|cell| cell.passed).count();
    let passed = passed_count == cells.len();
    let mut evidence = format!("{passed_count}/{} matrix cells passed", cells.len());
    if !passed {
        let failed_cells = cells
            .iter()
            .filter(|cell| !cell.passed)
            .map(|cell| {
                let label = matrix
                    .keys()
                    .map(|name| format!("{name}={}", cell.env[name]))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{label} ({})", cell.evidence)
            })
            .collect::<Vec<_>>()
            .join("; ");
        evidence.push_str("; failed cells: ");
        evidence.push_str(&failed_cells);
    }

    Ok(CommandCheckResult {
        id: assertion.id.clone(),
        passed,
        evidence,
        expected_exit_code: assertion.expect_exit_code,
        actual_exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        cells: Some(cells),
        definition_digest: Some(definition_digest(assertion)),
    })
}

fn validate_command_environment(assertion: &AssertionCommandCheck) -> Result<(), PipelineError> {
    for (name, value) in assertion.env.as_ref().into_iter().flatten() {
        validate_command_environment_name(&assertion.id, "env", name)?;
        validate_command_environment_value(&assertion.id, "env", name, value)?;
    }
    for (name, values) in assertion.matrix.as_ref().into_iter().flatten() {
        validate_command_environment_name(&assertion.id, "matrix", name)?;
        for value in values {
            validate_command_environment_value(&assertion.id, "matrix", name, value)?;
        }
    }
    Ok(())
}

fn validate_command_environment_name(
    assertion_id: &str,
    field: &str,
    name: &str,
) -> Result<(), PipelineError> {
    if name.is_empty() || name.contains('=') || name.contains('\0') {
        return Err(PipelineError::Message(format!(
            "command_check '{assertion_id}': {field} environment variable name must be non-empty and contain neither '=' nor NUL: {name:?}"
        )));
    }
    Ok(())
}

fn validate_command_environment_value(
    assertion_id: &str,
    field: &str,
    name: &str,
    value: &str,
) -> Result<(), PipelineError> {
    if value.contains('\0') {
        return Err(PipelineError::Message(format!(
            "command_check '{assertion_id}': {field} environment variable {name:?} value must not contain NUL"
        )));
    }
    Ok(())
}

fn matrix_environments(assertion: &AssertionCommandCheck) -> Vec<BTreeMap<String, String>> {
    let mut cells = vec![assertion.env.clone().unwrap_or_default()];
    for (name, values) in assertion.matrix.as_ref().into_iter().flatten() {
        let mut expanded = Vec::new();
        for cell in cells {
            for value in values {
                let mut env = cell.clone();
                env.insert(name.clone(), value.clone());
                expanded.push(env);
            }
        }
        cells = expanded;
    }
    cells
}

fn execute_command_check_cell(
    assertion: &AssertionCommandCheck,
    eval_root: &Path,
    env: BTreeMap<String, String>,
) -> Result<CommandCheckCellResult, PipelineError> {
    let mut command = Command::new("sh");
    command.arg("-c").arg(&assertion.command);

    command.current_dir(eval_root);
    // The task root defines repository discovery for runner-owned checks.
    // Explicit eval-author overlays are applied afterward, so a check may
    // intentionally restore a routing variable when that is part of the test.
    clear_git_environment(&mut command);
    command.envs(&env);
    let output = command.output();

    let output = output.map_err(|error| {
        PipelineError::Message(format!(
            "could not launch the POSIX shell for command_check '{}': {error}",
            assertion.id
        ))
    })?;
    let complete_stdout = String::from_utf8_lossy(&output.stdout);
    let complete_stderr = String::from_utf8_lossy(&output.stderr);
    let actual_exit_code = output.status.code();
    let mut failures = Vec::new();

    match actual_exit_code {
        Some(actual) if actual != assertion.expect_exit_code => failures.push(format!(
            "expected exit code {}, got {actual}",
            assertion.expect_exit_code
        )),
        Some(_) => {}
        None => failures.push(termination_evidence(&output.status)),
    }

    if let Some(pattern) = &assertion.expect_stdout {
        match Regex::new(pattern) {
            Ok(regex) if !regex.is_match(&complete_stdout) => {
                failures.push(format!(
                    "stdout did not match expect_stdout regex {pattern:?}"
                ));
            }
            Ok(_) => {}
            Err(error) => {
                failures.push(format!("invalid expect_stdout regex {pattern:?}: {error}"))
            }
        }
    }

    let passed = failures.is_empty();
    let evidence = if passed {
        match &assertion.expect_stdout {
            Some(pattern) => format!(
                "exit code matched {}; stdout matched regex {pattern:?}",
                assertion.expect_exit_code
            ),
            None => format!("exit code matched {}", assertion.expect_exit_code),
        }
    } else {
        failures.join("; ")
    };

    Ok(CommandCheckCellResult {
        env,
        passed,
        evidence,
        actual_exit_code,
        stdout: truncate_diagnostic(&complete_stdout),
        stderr: truncate_diagnostic(&complete_stderr),
    })
}

/// The evidence line for a child that ended without an exit code. Split from
/// [`termination_evidence`] so tests can pin the wording independently of
/// signal extraction.
fn termination_message(signal: Option<i32>) -> String {
    match signal {
        Some(signal) => format!("command terminated by signal {signal}"),
        None => "command terminated without an exit code".to_string(),
    }
}

fn termination_evidence(status: &ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    termination_message(status.signal())
}

fn truncate_diagnostic(value: &str) -> String {
    if value.len() <= DIAGNOSTIC_LIMIT {
        return value.to_string();
    }
    let mut end = DIAGNOSTIC_LIMIT;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests;
