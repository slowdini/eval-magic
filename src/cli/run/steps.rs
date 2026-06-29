//! `ingest` / `finalize` — fixed-order chains over the post-dispatch stages.
//!
//! The eval loop has exactly two points where only the
//! in-harness agent can act (dispatching eval subagents, dispatching judge
//! subagents). Everything between them is mechanical, so each stretch is one
//! command: `ingest` runs the post-dispatch chain and stops at the judge
//! hand-off; `finalize` runs the post-judge chain. No workspace-state inference —
//! each always runs the same steps in the same order, and every sub-step keeps
//! its own skip-if-done guard, so re-running after a fix is safe.
//!
//! A step is modeled as pure data (a [`StepCommand`]) so the chain wiring is
//! testable without executing anything. [`run_steps`] takes the runner as a
//! parameter; the production runner — which maps each [`StepKind`] to its stage
//! handler — lives in [`crate::cli`] alongside those handlers.

use crate::core::{Harness, RunMode};

/// Which post-dispatch stage a [`StepCommand`] runs. The production runner
/// matches on this to call the corresponding handler; tests assert on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    RecordRuns,
    FillTranscripts,
    DetectStrayWrites,
    Grade { finalize: bool },
    Aggregate,
}

/// One chain step: a label (for logging + failure reporting), the stage to run,
/// and the resolved flags to run it with. Pure data — no closure — so the flag
/// wiring is inspectable in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepCommand {
    pub label: &'static str,
    pub kind: StepKind,
    pub skill_dir: Option<String>,
    pub skill: Option<String>,
    pub iteration: u32,
    pub harness: Harness,
    /// The run mode, re-derived at each stage. Round-trips through `CommonArgs`
    /// exactly like `harness`, so ingest sub-stages don't silently re-default it.
    pub run_mode: RunMode,
    pub workspace_dir: Option<String>,
}

/// Resolved inputs shared by every step of a chain.
#[derive(Debug, Clone)]
pub struct StepParams<'a> {
    pub skill_dir: Option<&'a str>,
    pub skill: Option<&'a str>,
    pub iteration: u32,
    pub harness: Harness,
    pub run_mode: RunMode,
    pub workspace_dir: Option<&'a str>,
}

impl Default for StepParams<'_> {
    fn default() -> Self {
        Self {
            skill_dir: None,
            skill: None,
            iteration: 0,
            harness: Harness::ClaudeCode,
            run_mode: RunMode::Hybrid,
            workspace_dir: None,
        }
    }
}

impl StepParams<'_> {
    fn step(&self, label: &'static str, kind: StepKind) -> StepCommand {
        StepCommand {
            label,
            kind,
            skill_dir: self.skill_dir.map(str::to_string),
            skill: self.skill.map(str::to_string),
            iteration: self.iteration,
            harness: self.harness,
            run_mode: self.run_mode,
            workspace_dir: self.workspace_dir.map(str::to_string),
        }
    }
}

/// The ingest chain: record-runs → fill-transcripts → detect-stray-writes →
/// grade. Each stage reads its transcript from each task's `outputs/` events
/// file.
pub fn build_ingest_commands(p: &StepParams) -> Vec<StepCommand> {
    vec![
        p.step("record-runs", StepKind::RecordRuns),
        p.step("fill-transcripts", StepKind::FillTranscripts),
        p.step("detect-stray-writes", StepKind::DetectStrayWrites),
        p.step("grade", StepKind::Grade { finalize: false }),
    ]
}

/// The finalize chain: grade --finalize → aggregate.
pub fn build_finalize_commands(p: &StepParams) -> Vec<StepCommand> {
    vec![
        p.step("grade --finalize", StepKind::Grade { finalize: true }),
        p.step("aggregate", StepKind::Aggregate),
    ]
}

/// Run `steps` in order via `run`, stopping at the first failure and returning
/// its label (`None` = all succeeded). A failure must halt the chain: grade's
/// `__skill_invoked` code-check silently degrades to an LLM judge when
/// `tool_invocations` is missing, so grading after a failed record/fill step
/// would quietly lose the deterministic check.
pub fn run_steps<E>(
    steps: &[StepCommand],
    mut run: impl FnMut(&StepCommand) -> Result<(), E>,
) -> Option<&'static str> {
    for step in steps {
        println!("\n── {} ──", step.label);
        if run(step).is_err() {
            return Some(step.label);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> StepParams<'static> {
        StepParams {
            skill_dir: Some("/skills"),
            skill: Some("mr-review"),
            iteration: 2,
            ..Default::default()
        }
    }

    #[test]
    fn ingest_runs_record_fill_stray_grade_in_order() {
        let steps = build_ingest_commands(&params());
        assert_eq!(
            steps.iter().map(|s| s.label).collect::<Vec<_>>(),
            vec![
                "record-runs",
                "fill-transcripts",
                "detect-stray-writes",
                "grade"
            ]
        );
        assert_eq!(
            steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
            vec![
                StepKind::RecordRuns,
                StepKind::FillTranscripts,
                StepKind::DetectStrayWrites,
                StepKind::Grade { finalize: false },
            ]
        );
        // Every step carries the shared flags.
        for s in &steps {
            assert_eq!(s.skill_dir.as_deref(), Some("/skills"));
            assert_eq!(s.skill.as_deref(), Some("mr-review"));
            assert_eq!(s.iteration, 2);
        }
    }

    #[test]
    fn ingest_threads_harness_through_every_step() {
        let steps = build_ingest_commands(&StepParams {
            skill_dir: Some("/skills"),
            skill: Some("mr-review"),
            iteration: 2,
            harness: Harness::Codex,
            run_mode: RunMode::Hybrid,
            ..Default::default()
        });
        assert_eq!(
            steps.iter().map(|s| s.label).collect::<Vec<_>>(),
            vec![
                "record-runs",
                "fill-transcripts",
                "detect-stray-writes",
                "grade"
            ]
        );
        assert!(steps.iter().all(|s| s.harness == Harness::Codex));
    }

    #[test]
    fn finalize_runs_grade_finalize_then_aggregate() {
        let steps = build_finalize_commands(&StepParams {
            skill_dir: Some("/skills"),
            skill: Some("mr-review"),
            iteration: 2,
            ..Default::default()
        });
        assert_eq!(
            steps.iter().map(|s| s.label).collect::<Vec<_>>(),
            vec!["grade --finalize", "aggregate"]
        );
        assert_eq!(steps[0].kind, StepKind::Grade { finalize: true });
        assert_eq!(steps[1].kind, StepKind::Aggregate);
    }

    fn synthetic(label: &'static str) -> StepCommand {
        StepCommand {
            label,
            kind: StepKind::Aggregate,
            skill_dir: None,
            skill: None,
            iteration: 0,
            harness: Harness::ClaudeCode,
            run_mode: RunMode::Hybrid,
            workspace_dir: None,
        }
    }

    #[test]
    fn run_steps_stops_at_first_failure() {
        let steps = [synthetic("a"), synthetic("b"), synthetic("c")];
        let mut ran: Vec<&str> = Vec::new();
        let failed = run_steps(&steps, |step| {
            ran.push(step.label);
            if step.label == "b" { Err(()) } else { Ok(()) }
        });
        assert_eq!(ran, vec!["a", "b"]); // c never runs after b fails
        assert_eq!(failed, Some("b"));
    }

    #[test]
    fn run_steps_reports_no_failure_on_success() {
        let steps = [synthetic("a"), synthetic("b")];
        let failed = run_steps(&steps, |_| Ok::<(), ()>(()));
        assert_eq!(failed, None);
    }

    #[test]
    fn direct_skill_context_keeps_skill_dir_absent() {
        let steps = build_ingest_commands(&StepParams {
            skill: Some("/skills/mr-review"),
            iteration: 1,
            ..Default::default()
        });

        assert!(steps.iter().all(|s| s.skill_dir.is_none()));
        assert!(
            steps
                .iter()
                .all(|s| s.skill.as_deref() == Some("/skills/mr-review"))
        );
    }

    #[test]
    fn inferred_seeded_context_keeps_skill_absent() {
        let steps = build_finalize_commands(&StepParams {
            skill_dir: Some("/skills"),
            iteration: 1,
            ..Default::default()
        });

        assert!(steps.iter().all(|s| s.skill.as_deref().is_none()));
        assert!(
            steps
                .iter()
                .all(|s| s.skill_dir.as_deref() == Some("/skills"))
        );
    }
}
