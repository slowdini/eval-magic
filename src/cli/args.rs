//! The `clap` derive command tree: the top-level parser, the shared/per-command
//! argument groups, and the subcommand enum.
//!
//! Mirrors the manual flag parsing of eval-runner's `run.ts`/`cli.ts`. Flags are
//! intentionally permissive (mostly optional); each handler tightens them as
//! behavior lands (see the handlers in [`super::commands`]).

use clap::{Args, Parser, Subcommand};

use crate::core::Harness;

/// Run skill evals — measure whether an agent skill actually shifts behavior.
///
/// An eval dispatches a fresh subagent twice per test case — once with the skill
/// loaded, once without (or old version vs. new) — and grades both outputs against
/// assertions. The pass-rate delta tells you whether the skill is worth shipping.
/// This CLI builds the workspace, stages skills for discovery, generates dispatch
/// prompts, assembles run records from transcripts, grades, and aggregates; your
/// agent harness supplies the one thing it never does itself: dispatching the
/// subagents.
///
/// The run loop is one canonical workflow in both modes:
///
///   run → dispatch agents → ingest → dispatch judges → finalize → teardown
///
/// Every command takes two required flags: `--skill-dir` (the directory *holding*
/// skill folders — it is the eval's test environment; every skill in it gets
/// staged) and `--skill` (which folder to evaluate). With no subcommand, the
/// default action is `run`.
#[derive(Debug, Parser)]
#[command(
    name = "skill-eval",
    version,
    about = "Run skill evals — measure whether an agent skill actually shifts behavior.",
    after_help = super::help::AFTER_HELP
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Flags shared by most subcommands. Ported from the manual `flag()` parsing in
/// eval-runner's `run.ts`/`cli.ts`.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Directory containing the skill(s) under evaluation (required).
    ///
    /// This directory IS the eval's test environment: the skill-under-test is
    /// staged under a unique slug, and every *other* skill folder inside it is
    /// staged under its natural name so cross-references resolve. If it holds only
    /// your skill, the eval runs in isolation — copy or symlink siblings in to
    /// stage them.
    #[arg(long)]
    pub skill_dir: Option<String>,
    /// Skill name under evaluation — the subdirectory of `--skill-dir` (required).
    #[arg(long)]
    pub skill: Option<String>,
    /// Iteration number for post-dispatch steps.
    #[arg(long)]
    pub iteration: Option<u32>,
    /// Comparison mode: `new-skill` (with vs. without) or `revision` (old vs. new).
    ///
    /// Mode A (`new-skill`) validates a brand-new skill against baseline behavior
    /// with no skill loaded. Mode B (`revision`) tests a language change to an
    /// existing skill: snapshot the old `SKILL.md` (see `snapshot`), then run both
    /// variants against the same prompts. `revision` requires `--baseline`.
    #[arg(long)]
    pub mode: Option<String>,
    /// Target harness: `claude-code` (default) or `codex`.
    ///
    /// Claude Code is the fully wired harness. Codex stages skills under
    /// `.agents/skills` and reads each task's `outputs/codex-events.jsonl` instead
    /// of a subagents dir; `--guard` and `--plan-mode` are Claude-Code-only.
    #[arg(long)]
    pub harness: Option<Harness>,
    /// Workspace directory (defaults to `<cwd>/skills-workspace`).
    ///
    /// The artifact root. Pass the same value to every command of a run, including
    /// `teardown`.
    #[arg(long)]
    pub workspace_dir: Option<String>,
    /// Subagents transcript dir (Claude Code only), e.g.
    /// `~/.claude/projects/<slug>/<session-id>/subagents/`.
    ///
    /// Where Claude Code persisted subagent transcripts. `ingest`/`record-runs`/
    /// `fill-transcripts` read it to populate `tool_invocations`, tokens, and
    /// duration. Not used for Codex, which reads `outputs/codex-events.jsonl`.
    #[arg(long)]
    pub subagents_dir: Option<String>,
    /// Restrict to these eval ids (comma-separated).
    ///
    /// Mutually exclusive with `--skip`; every named id must exist or the run
    /// aborts with the available ids listed. For cost-conscious reduced-set runs
    /// without editing `evals.json`.
    #[arg(long)]
    pub only: Option<String>,
    /// Skip these eval ids (comma-separated). Mutually exclusive with `--only`.
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
    ///
    /// Reads the SKILL.md + sibling assets (excluding `evals/`) straight from git
    /// without touching the working tree — the edit-first Mode B order: edit, then
    /// `snapshot --ref HEAD`. Without `--ref`, snapshot reads the working tree.
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
    ///
    /// The snapshot label to use as the `old_skill` arm in revision mode (see
    /// `snapshot`).
    #[arg(long)]
    pub baseline: Option<String>,
    /// SessionStart-equivalent bootstrap file inlined into each dispatch.
    ///
    /// A Markdown file prepended verbatim to every dispatch prompt inside a
    /// `<session-start-context>` block — product-specific framing a SessionStart
    /// hook would inject. It does NOT enumerate skills (the auto-built
    /// available-skills block is the single source of the skill list). Omit it and
    /// dispatches carry only that inventory.
    #[arg(long)]
    pub bootstrap: Option<String>,
    /// Build the workspace but skip guard install and stop before next steps.
    #[arg(long)]
    pub dry_run: bool,
    /// Inline each condition's SKILL.md into the dispatch prompt instead of
    /// staging it under the harness skills dir.
    ///
    /// For harnesses without project-local skill discovery. Forces the LLM-judge
    /// meta-check tier and does not inline sibling skills or sibling asset files,
    /// so multi-file skills need the staged (default) path.
    #[arg(long)]
    pub no_stage: bool,
    /// Arm the write guard (PreToolUse hook) for the dispatch window.
    ///
    /// Stages a `PreToolUse` hook that *blocks* subagent writes/installs outside
    /// the eval sandbox while dispatches run. Wired for Claude Code today and the
    /// default posture there — arm it unless the user opts out. The marker
    /// auto-expires after 6h and is torn down at the next run; while armed the hook
    /// fires on your own tool calls too. Unguarded, stray writes are only *detected*
    /// after the fact by `detect-stray-writes`, never blocked.
    #[arg(long)]
    pub guard: bool,
    /// Stage the skill-under-test under this verbatim name instead of the
    /// conspicuous `slow-powers-eval-…` slug.
    ///
    /// For name-confound experiments. Single-staging-condition modes only; refuses
    /// to clobber an existing dir; registered for next-run cleanup.
    #[arg(long)]
    pub stage_name: Option<String>,
    /// Inject the harness's plan-mode profile as an operating-context layer.
    ///
    /// Injects the harness's verbatim plan-mode procedure
    /// (`profiles/<harness>/plan-mode.md`) as a `<system-reminder>` in every
    /// dispatch, identical across arms. Opt-in, for plan-mode-relevant skills; only
    /// the Claude Code profile ships today, and a harness with no profile aborts.
    /// It is text the subagent reads, not a real injected mode.
    #[arg(long)]
    pub plan_mode: bool,
}

/// Every subcommand ported from eval-runner. Names match the original CLI.
#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    /// Build dispatches and run evals (the default action).
    ///
    /// Builds the iteration workspace, snapshots the `SKILL.md`, stages skills, and
    /// emits `dispatch.json` (machine-readable) alongside `dispatch-manifest.md`
    /// (human-readable). Your agent then dispatches each task as a fresh subagent.
    Run(RunArgs),
    /// Snapshot a workspace baseline.
    ///
    /// Snapshots the skill as a Mode B baseline under
    /// `<workspace>/<skill>/snapshots/<label>/`. Snapshots persist across
    /// iterations; delete them by hand when no longer needed.
    Snapshot(SnapshotArgs),
    /// Tear down a workspace.
    ///
    /// Disarms the guard, removes the staged skill set, and reclaims the workspace
    /// artifacts that are safe to delete. Run it at the end of a run.
    Teardown(CommonArgs),
    /// Disarm the write guard.
    ///
    /// Removes only the write guard (e.g. mid-run, before hand-editing files the
    /// guard would block). The full `teardown` removes the guard AND the staged
    /// skill set.
    TeardownGuard(CommonArgs),
    /// Ingest recorded transcripts into run records.
    ///
    /// Fixed-order chain: record-runs → fill-transcripts → detect-stray-writes →
    /// grade. Assembles each task's `run.json` + `timing.json`, scans for stray
    /// writes, grades `transcript_check` assertions, then stops at the judge
    /// hand-off, listing a judge task per `llm_judge` assertion. Requires
    /// `--iteration`; Claude Code also needs `--subagents-dir`, while Codex reads
    /// each task's `outputs/codex-events.jsonl`. Re-running after a fix is safe —
    /// every sub-step skips work already done.
    Ingest(CommonArgs),
    /// Finalize grading after judge responses are in.
    ///
    /// Fixed-order chain: grade `--finalize` → aggregate. Merges the judge verdicts
    /// and writes `benchmark.json`. Requires `--iteration`.
    Finalize(CommonArgs),
    /// Assemble run records from a dispatch and its transcripts.
    ///
    /// Assembles a schema-valid `run.json` and backfills `timing.json` for every
    /// task in a runner-built iteration, from `dispatch.json` +
    /// `outputs/final-message.md` + the persisted transcript. Never clobbers
    /// existing records without `--overwrite`; transcript-derived timing carries
    /// `"source": "transcript"`. Folded into `ingest`.
    RecordRuns(CommonArgs),
    /// Populate tool invocations from persisted transcripts.
    ///
    /// Matches each `(eval, condition)` to a subagent transcript by description and
    /// populates `tool_invocations` in `run.json`. Subsumed by `record-runs` for
    /// runner-built iterations; still the tool for filling a pre-existing (hand- or
    /// agent-written) `run.json`.
    FillTranscripts(CommonArgs),
    /// Detect writes outside the sandbox output boundary.
    ///
    /// Scans each run's `tool_invocations` and writes `stray-writes.json`: write
    /// tools targeting paths outside the run's outputs dir (violations), mutating
    /// Bash heuristics (warnings), and live-source reads (an arm that read the live
    /// skill instead of its staged copy). `aggregate` lifts all three into
    /// `benchmark.json`'s `validity_warnings`.
    DetectStrayWrites(CommonArgs),
    /// Grade run records (transcript checks + LLM-judge task emission).
    ///
    /// Evaluates `transcript_check` assertions directly (regex against
    /// `tool_invocations`) and emits judge-task files for `llm_judge` assertions;
    /// with `--finalize`, merges judge responses into per-run `grading.json`.
    ///
    /// Injects the `__skill_invoked` meta-check — did the skill actually influence
    /// behavior? It has two tiers, chosen automatically per run: code-based (where
    /// the staged slug + transcript are available, as on Claude Code, it checks the
    /// transcript for a `Skill` call matching the eval slug — deterministic and
    /// free) and an LLM-judge fallback (where transcripts aren't available, a judge
    /// compares the final message against the SKILL.md for behavioral fingerprints).
    /// The meta-check does not count toward the substantive `pass_rate`.
    Grade(GradeArgs),
    /// Aggregate before/after benchmark deltas.
    ///
    /// Reads grading + timing from an iteration and writes `benchmark.json` with
    /// pass-rate / duration / token stats per condition, the delta, and
    /// `validity_warnings`.
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
