//! The `clap` derive command tree: the top-level parser, the shared/per-command
//! argument groups, and the subcommand enum.
//!
//! Mirrors the manual flag parsing of eval-runner's `run.ts`/`cli.ts`. Flags are
//! intentionally permissive (mostly optional); each handler tightens them as
//! behavior lands (see the handlers in [`super::commands`]).

use clap::{Args, Parser, Subcommand};

use crate::core::Harness;

/// Top-level CLI. With no subcommand, the default action is `run`.
#[derive(Debug, Parser)]
#[command(
    name = "skill-eval",
    version,
    about = "Run skill evals — measure whether an agent skill actually shifts behavior."
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Flags shared by most subcommands. Ported from the manual `flag()` parsing in
/// eval-runner's `run.ts`/`cli.ts`.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Directory containing the skill under evaluation.
    #[arg(long)]
    pub skill_dir: Option<String>,
    /// Skill name under evaluation.
    #[arg(long)]
    pub skill: Option<String>,
    /// Iteration number for post-dispatch steps.
    #[arg(long)]
    pub iteration: Option<u32>,
    /// Comparison mode: `new-skill` (with vs. without) or `revision` (old vs. new).
    #[arg(long)]
    pub mode: Option<String>,
    /// Target harness.
    #[arg(long)]
    pub harness: Option<Harness>,
    /// Workspace directory (defaults to `<cwd>/skills-workspace`).
    #[arg(long)]
    pub workspace_dir: Option<String>,
    /// Subagents transcript dir (Claude Code only), e.g.
    /// `~/.claude/projects/<slug>/<session-id>/subagents/`.
    #[arg(long)]
    pub subagents_dir: Option<String>,
    /// Restrict to these eval ids (comma-separated).
    #[arg(long)]
    pub only: Option<String>,
    /// Skip these eval ids (comma-separated).
    #[arg(long)]
    pub skip: Option<String>,
    /// Replace existing records rather than erroring.
    #[arg(long)]
    pub overwrite: bool,
}

/// `validate` only needs to know where to look.
#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Directory whose `evals.json` files should be validated.
    #[arg(long)]
    pub skill_dir: Option<String>,
}

/// `grade` adds a finalize flag on top of the common set.
#[derive(Debug, Args)]
pub struct GradeArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Merge judge responses instead of emitting judge tasks.
    #[arg(long)]
    pub finalize: bool,
}

/// `snapshot` adds a label and an optional git ref on top of the common set.
#[derive(Debug, Args)]
pub struct SnapshotArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Label for the snapshot (its directory name under `snapshots/`).
    #[arg(long)]
    pub label: Option<String>,
    /// Snapshot the skill as it existed at this git ref instead of the working
    /// tree. (`ref` is a Rust keyword, so the field is `reference`.)
    #[arg(long = "ref")]
    pub reference: Option<String>,
}

/// `promote-baseline` adds provenance flags (label + operator-declared models)
/// on top of the common set.
#[derive(Debug, Args)]
pub struct PromoteBaselineArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Provenance label recorded in `BASELINE.md`.
    #[arg(long)]
    pub label: Option<String>,
    /// Operator-declared agent model, recorded in `BASELINE.md`.
    #[arg(long)]
    pub agent_model: Option<String>,
    /// Operator-declared judge model, recorded in `BASELINE.md`.
    #[arg(long)]
    pub judge_model: Option<String>,
}

/// `run` adds the build-time flags (mode/baseline selection, staging toggles,
/// guard, plan-mode, bootstrap) on top of the common set.
#[derive(Debug, Args)]
pub struct RunArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Baseline snapshot label (required in `--mode revision`).
    #[arg(long)]
    pub baseline: Option<String>,
    /// SessionStart-equivalent bootstrap file inlined into each dispatch.
    #[arg(long)]
    pub bootstrap: Option<String>,
    /// Build the workspace but skip guard install and stop before next steps.
    #[arg(long)]
    pub dry_run: bool,
    /// Inline each condition's SKILL.md into the dispatch prompt instead of
    /// staging it under the harness skills dir.
    #[arg(long)]
    pub no_stage: bool,
    /// Arm the write guard (PreToolUse hook) for the dispatch window.
    #[arg(long)]
    pub guard: bool,
    /// Stage the skill-under-test under this verbatim name instead of the
    /// conspicuous `slow-powers-eval-…` slug.
    #[arg(long)]
    pub stage_name: Option<String>,
    /// Inject the harness's plan-mode profile as an operating-context layer.
    #[arg(long)]
    pub plan_mode: bool,
}

/// Every subcommand ported from eval-runner. Names match the original CLI.
#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    /// Build dispatches and run evals (the default action).
    Run(RunArgs),
    /// Snapshot a workspace baseline.
    Snapshot(SnapshotArgs),
    /// Tear down a workspace.
    Teardown(CommonArgs),
    /// Disarm the write guard.
    TeardownGuard(CommonArgs),
    /// Ingest recorded transcripts into run records.
    Ingest(CommonArgs),
    /// Finalize grading after judge responses are in.
    Finalize(CommonArgs),
    /// Assemble run records from a dispatch and its transcripts.
    RecordRuns(CommonArgs),
    /// Populate tool invocations from persisted transcripts.
    FillTranscripts(CommonArgs),
    /// Detect writes outside the sandbox output boundary.
    DetectStrayWrites(CommonArgs),
    /// Grade run records (transcript checks + LLM-judge task emission).
    Grade(GradeArgs),
    /// Aggregate before/after benchmark deltas.
    Aggregate(CommonArgs),
    /// Promote a benchmark + gradings into a committed baseline.
    PromoteBaseline(PromoteBaselineArgs),
    /// Validate `evals.json` files against the bundled schemas.
    Validate(ValidateArgs),
    /// Internal PreToolUse hook entry point. Invoked by the installed write-guard
    /// hook as `skill-eval guard <marker>`, not by users; hidden from help.
    #[command(hide = true)]
    Guard {
        /// Path to the guard marker file. Defaults to
        /// `<cwd>/.claude/skills/.slow-powers-eval-guard.json`.
        marker: Option<String>,
    },
}
