//! The batch dispatch driver behind `eval-magic dispatch`.
//!
//! `run` prepares a plan; this executes it. One command owns the whole batch —
//! concurrency, per-task failure accounting, and deciding what a rerun may skip
//! — so no operator drives tasks one at a time from a generated recipe.

pub mod judges;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, anyhow};
use serde::Deserialize;

use crate::adapters::descriptor::finalize_descriptor;
use crate::adapters::descriptor_adapter::DescriptorAdapter;
use crate::cli::run::conversation::{TaskOutcome, run_task};
use crate::cli::run::dispatch::DispatchTask;
use crate::core::{ConversationStopReason, posix_shell, validate_agent_environment_entry};

/// The run-time half of `dispatch.json`: everything the driver needs to execute
/// a task, frozen at plan time so a dispatch is reproducible from the workspace
/// alone.
#[derive(Debug, Deserialize)]
pub struct DispatchEnvelope {
    #[serde(default)]
    pub guard: bool,
    #[serde(default)]
    pub agent_model: Option<String>,
    #[serde(default)]
    pub agent_env: BTreeMap<String, String>,
    pub harness_descriptor: serde_json::Value,
    pub tasks: Vec<DispatchTask>,
}

impl DispatchEnvelope {
    /// Read and validate a plan. The `agent_env` check happens here rather than
    /// per task so a malformed environment fails before anything is dispatched.
    pub fn load(dispatch_path: &Path) -> anyhow::Result<Self> {
        let raw = fs::read_to_string(dispatch_path)
            .with_context(|| format!("failed to read {}", dispatch_path.display()))?;
        let envelope: Self = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", dispatch_path.display()))?;
        for (name, value) in &envelope.agent_env {
            validate_agent_environment_entry(name, value).map_err(|message| {
                anyhow!(
                    "invalid agent_env in {}: {message}",
                    dispatch_path.display()
                )
            })?;
        }
        Ok(envelope)
    }

    /// Build an adapter over the frozen descriptor. Each dispatch worker calls
    /// this for itself: the adapter is cheap to build from a value both threads
    /// only read, which keeps the pool from having to share one.
    pub fn adapter(&self, dispatch_path: &Path) -> anyhow::Result<DescriptorAdapter> {
        let descriptor = finalize_descriptor(
            &self.harness_descriptor,
            &format!("{}#harness_descriptor", dispatch_path.display()),
        )?;
        Ok(DescriptorAdapter::from_descriptor(descriptor))
    }
}

/// What one task did, paired with the identity to report it under. Every task
/// produces one of these, including the ones that failed, which is what lets a
/// batch finish rather than abort on the first bad dispatch.
#[derive(Debug)]
pub struct TaskReport {
    pub description: String,
    pub result: Result<TaskOutcome, String>,
}

/// The tally a dispatch prints when it finishes, in task order.
#[derive(Debug, Default)]
pub struct DispatchSummary {
    pub reports: Vec<TaskReport>,
}

impl DispatchSummary {
    fn count(&self, matching: impl Fn(&TaskOutcome) -> bool) -> usize {
        self.reports
            .iter()
            .filter(|report| report.result.as_ref().is_ok_and(&matching))
            .count()
    }

    pub fn completed(&self) -> usize {
        self.count(|outcome| matches!(outcome, TaskOutcome::Completed { .. }))
    }

    pub fn stopped(&self) -> usize {
        self.count(|outcome| matches!(outcome, TaskOutcome::Stopped { .. }))
    }

    pub fn skipped(&self) -> usize {
        self.count(|outcome| matches!(outcome, TaskOutcome::SkippedExisting))
    }

    pub fn timed_out(&self) -> usize {
        self.count(|outcome| matches!(outcome, TaskOutcome::TimedOut { .. }))
    }

    pub fn failed(&self) -> usize {
        self.reports
            .iter()
            .filter(|report| report.result.is_err())
            .count()
    }

    /// Warnings naming every task whose result needs a second look. A scripted
    /// gate stop is not one: the script said what to do and the run did it.
    ///
    /// A responder stop is. Nothing failed and nothing needs rerunning, but the
    /// conversation ended with the task unfinished, so the record it leaves is
    /// weaker evidence than a completed run and must not be read as one.
    pub fn warnings(&self) -> Vec<String> {
        self.reports
            .iter()
            .filter_map(|report| match &report.result {
                Err(reason) => Some(format!("{} failed: {reason}", report.description)),
                Ok(TaskOutcome::TimedOut { round }) => Some(format!(
                    "{} timed out in round {round}; its environment holds whatever the \
                     agent finished before the deadline",
                    report.description
                )),
                Ok(TaskOutcome::Stopped {
                    reason: Some(ConversationStopReason::ResponderCannotAnswer),
                    ..
                }) => Some(format!(
                    "{} stopped: the responder could not answer the agent's question, so the run \
                     ended mid-task. Read the last assistant message under its outputs before \
                     trusting this data point.",
                    report.description
                )),
                Ok(TaskOutcome::Stopped {
                    reason: Some(ConversationStopReason::MaxTurnsReached),
                    ..
                }) => Some(format!(
                    "{} stopped at the responder's max_turns bound with the agent still asking, \
                     so the run ended mid-task. Raise max_turns or read the transcript before \
                     trusting this data point.",
                    report.description
                )),
                Ok(_) => None,
            })
            .collect()
    }

    /// Tasks that produced no usable result. Drives the exit status: a script
    /// has to be able to tell a clean batch from one that needs a rerun.
    pub fn unusable(&self) -> usize {
        self.failed() + self.timed_out()
    }

    /// The headline tally line.
    pub fn tally(&self) -> String {
        format!(
            "{} completed, {} stopped, {} timed out, {} failed, {} skipped",
            self.completed(),
            self.stopped(),
            self.timed_out(),
            self.failed(),
            self.skipped()
        )
    }
}

/// Execute every task in `dispatch_path`, or just the selected indices.
pub fn command_dispatch(
    dispatch_path: &Path,
    task_indices: &[usize],
    overwrite: bool,
    timeout: Option<Duration>,
    jobs: usize,
) -> anyhow::Result<DispatchSummary> {
    let envelope = DispatchEnvelope::load(dispatch_path)?;
    let selected = select_tasks(&envelope, task_indices)?;
    // Fail before spawning anything if this host has no shell, and warm the
    // process-wide cache so workers only ever read it.
    posix_shell().map_err(|message| anyhow!("{message}"))?;
    // Every worker builds its own adapter, so a bad descriptor must fail once,
    // here, rather than identically in each thread.
    envelope.adapter(dispatch_path)?;

    let reports = run_pool(jobs, selected.len(), |slot| {
        let adapter = envelope.adapter(dispatch_path)?;
        let task = &envelope.tasks[selected[slot]];
        let result = run_task(
            &adapter,
            task,
            envelope.guard,
            envelope.agent_model.as_deref(),
            &envelope.agent_env,
            overwrite,
            timeout,
        )
        // A failed task is data about the campaign, not a reason to abandon the
        // rest of it. `{:#}` keeps anyhow's context chain in the reported reason.
        .map_err(|error| format!("{error:#}"));
        Ok(TaskReport {
            description: task.agent_description.clone(),
            result,
        })
    })?;
    Ok(DispatchSummary { reports })
}

/// Run `total` units of work across at most `jobs` threads, returning what each
/// produced in slot order — which worker finished first is not something a
/// report should depend on.
///
/// `work` is called once per slot and may run on any thread. An `Err` from it
/// aborts the batch: it means the runner itself could not proceed (a bad
/// descriptor, an unreachable shell), as distinct from a task that failed,
/// which `work` reports inside its own return value.
pub(crate) fn run_pool<T, F>(jobs: usize, total: usize, work: F) -> anyhow::Result<Vec<T>>
where
    T: Send + Describe,
    F: Fn(usize) -> anyhow::Result<T> + Sync,
{
    let cursor = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<T>>> = Mutex::new((0..total).map(|_| None).collect());

    std::thread::scope(|scope| -> anyhow::Result<()> {
        // Never more threads than there is work for them to do.
        let workers: Vec<_> = (0..jobs.min(total).max(1))
            .map(|_| {
                let (cursor, done, slots, work) = (&cursor, &done, &slots, &work);
                scope.spawn(move || -> anyhow::Result<()> {
                    loop {
                        let slot = cursor.fetch_add(1, Ordering::Relaxed);
                        if slot >= total {
                            return Ok(());
                        }
                        let produced = work(slot)?;
                        // One lock covers the progress line and the slot write,
                        // so concurrent workers cannot interleave mid-line.
                        let mut slots = slots.lock().expect("dispatch pool lock");
                        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                        println!("[{finished}/{total}] {}", produced.describe());
                        slots[slot] = Some(produced);
                    }
                })
            })
            .collect();
        // Joined explicitly: dropping the handles would discard a worker's
        // error, leaving a short batch that looks like a successful one.
        for worker in workers {
            worker
                .join()
                .map_err(|_| anyhow!("a dispatch worker panicked"))??;
        }
        Ok(())
    })?;

    Ok(slots
        .into_inner()
        .expect("dispatch pool lock")
        .into_iter()
        .flatten()
        .collect())
}

/// The one-line progress text a pooled unit of work reports when it finishes.
pub(crate) trait Describe {
    fn describe(&self) -> String;
}

impl Describe for TaskReport {
    fn describe(&self) -> String {
        match &self.result {
            Ok(outcome) => format!("{}: {}", self.description, outcome.summary()),
            // The reason follows as a ⚠ line, so this stays scannable.
            Err(_) => format!("{}: failed", self.description),
        }
    }
}

/// The task indices to run: every task by default, or exactly those requested.
/// An out-of-range index is an error before anything is dispatched, so a typo
/// cannot half-run a batch.
fn select_tasks(envelope: &DispatchEnvelope, requested: &[usize]) -> anyhow::Result<Vec<usize>> {
    if requested.is_empty() {
        return Ok((0..envelope.tasks.len()).collect());
    }
    let total = envelope.tasks.len();
    for index in requested {
        anyhow::ensure!(
            *index < total,
            "--task-index {index} is out of range for {total} task(s)"
        );
    }
    Ok(requested.to_vec())
}
