//! Stage 4 — `grade`.
//!
//! Decomposed into focused units:
//!
//! - [`transcript_check`] — grade a `transcript_check` assertion (regex over the
//!   run's tool invocations).
//! - [`judge_tasks`] — emit LLM judge tasks + the skill-invocation meta-check
//!   (`emit_judge_tasks`), the default mode.
//! - [`finalize`] — fold judge responses + transcript checks into `grading.json`
//!   (`--finalize` mode).
//!
//! Both modes operate over a shared [`GradeContext`] assembled by the CLI.

pub mod command_check;
pub mod diff_scope;
pub mod evidence;
pub mod finalize;
pub mod instrument;
pub mod judge_tasks;
pub mod stale_verdicts;
pub mod transcript_check;

use std::path::Path;

use crate::core::{AssertionSource, ConditionsRecord, EvalsConfig};

pub use command_check::{CommandCheckSummary, grade_command_checks};
pub use finalize::{FinalizeSummary, finalize};
pub use instrument::{GradingInstrument, resolve_grading_instrument};
pub use judge_tasks::{EmitSummary, check_skill_invoked_from_transcript, emit_judge_tasks};
pub use transcript_check::{
    ToolNaming, grade_transcript_check, grade_transcript_check_with_context,
};

/// The resolved inputs both grade modes read: the iteration directory, the
/// conditions manifest, and the validated evals config (its `skill_name` is the
/// one used in meta-check rubrics) — plus which `evals.json` supplied that
/// config's assertions, which `finalize` records in every `grading.json`.
pub struct GradeContext<'a> {
    pub iteration_dir: &'a Path,
    pub conditions: &'a ConditionsRecord,
    pub evals: &'a EvalsConfig,
    pub assertion_source: &'a AssertionSource,
}
