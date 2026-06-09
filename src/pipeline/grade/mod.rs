//! Stage 4 — `grade`.
//!
//! Ports `src/pipeline/grade.ts`, decomposed into focused units (the file was the
//! largest in the TypeScript pipeline):
//!
//! - [`transcript_check`] — grade a `transcript_check` assertion (regex over the
//!   run's tool invocations).
//! - [`judge_tasks`] — emit LLM judge tasks + the skill-invocation meta-check
//!   (`emit_judge_tasks`), the default mode.
//! - [`finalize`] — fold judge responses + transcript checks into `grading.json`
//!   (`--finalize` mode).
//!
//! Both modes operate over a shared [`GradeContext`] assembled by the CLI.

pub mod finalize;
pub mod judge_tasks;
pub mod transcript_check;

use std::path::Path;

use crate::core::{ConditionsRecord, EvalsConfig};

pub use finalize::{FinalizeSummary, finalize};
pub use judge_tasks::{EmitSummary, check_skill_invoked_from_transcript, emit_judge_tasks};
pub use transcript_check::grade_transcript_check;

/// The resolved inputs both grade modes read: the iteration directory, the
/// conditions manifest, and the validated evals config (its `skill_name` is the
/// one used in meta-check rubrics).
pub struct GradeContext<'a> {
    pub iteration_dir: &'a Path,
    pub conditions: &'a ConditionsRecord,
    pub evals: &'a EvalsConfig,
}
