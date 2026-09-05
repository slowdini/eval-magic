//! Runner-driven judge dispatch: the `--judges` half of `eval-magic dispatch`.
//!
//! A judge task is a one-shot dispatch like any other, so it reuses the
//! harness's `exec_template` with its placeholders bound differently — the
//! iteration directory instead of a private task env, the judge prompt instead
//! of the task prompt, and a per-task capture directory derived from the
//! response path. A harness therefore needs no judge-specific template: what
//! makes a judge a judge is the prompt `grade` wrote, not the command line.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;

use crate::adapters::descriptor_adapter::DescriptorAdapter;
use crate::adapters::harness::HarnessAdapter;
use crate::cli::run::conversation::render_dispatch_command;
use crate::cli::run::drive::DispatchEnvelope;
use crate::core::{ShellOutcome, posix_shell, run_in_posix_shell};

use super::{Describe, run_pool};

/// The parts of `judge-tasks.json` a dispatch needs.
#[derive(Debug, Deserialize)]
struct JudgeTasksFile {
    #[serde(default)]
    tasks: Vec<JudgeTask>,
}

#[derive(Debug, Deserialize)]
struct JudgeTask {
    eval_id: String,
    condition: String,
    assertion_id: String,
    #[serde(default)]
    model: Option<String>,
    response_path: String,
    dispatch_prompt_path: String,
    #[serde(default)]
    sample_index: Option<u32>,
    #[serde(default)]
    sample_count: Option<u32>,
}

impl JudgeTask {
    fn description(&self) -> String {
        let base = format!("{}:{}:{}", self.eval_id, self.condition, self.assertion_id);
        match (self.sample_index, self.sample_count) {
            (Some(index), Some(count)) => format!("{base}:sample-{index}-of-{count}"),
            _ => base,
        }
    }

    /// A verdict is present once its response file exists and is non-empty —
    /// the same test the shipped recipe applied with `[ -s "$response_path" ]`.
    fn verdict_present(&self) -> bool {
        std::fs::metadata(&self.response_path).is_ok_and(|meta| meta.len() > 0)
    }

    /// This task's private capture directory: the response path minus its
    /// `.json` suffix. Several assertions share one `judge-responses/`
    /// directory, so binding captures there would have each judge overwrite the
    /// previous one's transcript.
    fn capture_dir(&self) -> PathBuf {
        Path::new(&self.response_path).with_extension("")
    }
}

/// One judge dispatch's result, reported under the assertion it graded.
#[derive(Debug)]
struct JudgeReport {
    description: String,
    result: Result<(), String>,
}

impl Describe for JudgeReport {
    fn describe(&self) -> String {
        match &self.result {
            Ok(()) => format!("{}: dispatched", self.description),
            Err(_) => format!("{}: failed", self.description),
        }
    }
}

/// What a judge batch did. The verdict counts drive the exit status: a judge
/// batch is finished only once every verdict is on disk.
#[derive(Debug, Default)]
pub struct JudgeSummary {
    pub total: usize,
    pub present: usize,
    pub dispatched: usize,
    pub skipped: usize,
    pub failures: Vec<String>,
}

impl JudgeSummary {
    /// `N/M verdicts present` — the sentence the shipped recipe printed, kept
    /// word for word so an operator reads the same thing either way.
    pub fn verdict_line(&self) -> String {
        format!("{}/{} verdicts present", self.present, self.total)
    }

    /// Whether every judge task has a verdict and nothing failed on the way.
    pub fn complete(&self) -> bool {
        self.present == self.total && self.failures.is_empty()
    }
}

/// Dispatch every judge task that has no verdict yet.
pub fn command_dispatch_judges(
    iteration_dir: &Path,
    overwrite: bool,
    timeout: Option<Duration>,
    jobs: usize,
) -> anyhow::Result<JudgeSummary> {
    let judge_path = iteration_dir.join("judge-tasks.json");
    let raw = std::fs::read_to_string(&judge_path).with_context(|| {
        format!(
            "failed to read {} — run `eval-magic ingest` to emit judge tasks first",
            judge_path.display()
        )
    })?;
    let file: JudgeTasksFile = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", judge_path.display()))?;

    // A judge runs against the harness the campaign was planned with, so the
    // descriptor comes from the same frozen envelope the eval tasks used.
    let dispatch_path = iteration_dir.join("dispatch.json");
    let envelope = DispatchEnvelope::load(&dispatch_path)?;
    // Both fail once, here, rather than identically inside every worker.
    posix_shell().map_err(|message| anyhow::anyhow!("{message}"))?;
    envelope.adapter(&dispatch_path)?;

    let pending: Vec<&JudgeTask> = file
        .tasks
        .iter()
        .filter(|task| overwrite || !task.verdict_present())
        .collect();
    let mut summary = JudgeSummary {
        total: file.tasks.len(),
        skipped: file.tasks.len() - pending.len(),
        ..JudgeSummary::default()
    };

    let reports = run_pool(jobs, pending.len(), |slot| {
        let task = pending[slot];
        let adapter = envelope.adapter(&dispatch_path)?;
        Ok(JudgeReport {
            description: task.description(),
            result: dispatch_judge(&adapter, task, iteration_dir, &envelope.agent_env, timeout),
        })
    })?;

    summary.dispatched = reports.len();
    summary.failures = reports
        .into_iter()
        .filter_map(|report| {
            report
                .result
                .err()
                .map(|reason| format!("{}: {reason}", report.description))
        })
        .collect();
    // Counted from disk rather than from what was dispatched: a judge that ran
    // without writing its verdict has not produced one.
    summary.present = file
        .tasks
        .iter()
        .filter(|task| task.verdict_present())
        .count();
    Ok(summary)
}

/// Run one judge task through the harness's exec template.
fn dispatch_judge(
    adapter: &DescriptorAdapter,
    task: &JudgeTask,
    iteration_dir: &Path,
    agent_env: &BTreeMap<String, String>,
    timeout: Option<Duration>,
) -> Result<(), String> {
    // Guard arguments are deliberately off: a judge runs outside every guarded
    // task env, and the hook-trust bypass exists only for eval-agent dispatches
    // whose cwd actually contains the vetted guard hook.
    let template = adapter
        .cli_exec_command(false, task.model.as_deref(), agent_env)
        .ok_or_else(|| "harness declares no dispatch exec command".to_string())?;

    let capture_dir = task.capture_dir();
    std::fs::create_dir_all(&capture_dir)
        .map_err(|error| format!("failed to create {}: {error}", capture_dir.display()))?;

    let command = render_dispatch_command(
        &template,
        &iteration_dir.to_string_lossy(),
        &task.dispatch_prompt_path,
        &capture_dir,
    );
    match run_in_posix_shell(&command, iteration_dir, agent_env, timeout) {
        Ok(ShellOutcome::Exited(status)) if status.success() => Ok(()),
        Ok(ShellOutcome::Exited(status)) => Err(format!("judge command exited with {status}")),
        Ok(ShellOutcome::TimedOut) => Err("judge command timed out".to_string()),
        Err(message) => Err(message),
    }
}
