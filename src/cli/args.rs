//! The `clap` derive command tree: the top-level parser, the shared/per-command
//! argument groups, and the subcommand enum.
//!
//! Flags are intentionally permissive (mostly optional); each handler tightens
//! them to what it actually requires (see the handlers in [`super::commands`]).

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
/// The default target is the skill in the current directory. Pass
/// `--skill <path-or-name>` to select one skill from elsewhere. Pass
/// `--skill-dir <dir>` only when you want every other skill in that directory
/// staged as part of the eval environment. With no subcommand, the default
/// action is `run`.
#[derive(Debug, Parser)]
#[command(
    name = "eval-magic",
    version,
    about = "Run skill evals — measure whether an agent skill actually shifts behavior.",
    after_help = super::help::AFTER_HELP
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Flags shared by most subcommands.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Optional directory of skills to stage as the eval environment.
    ///
    /// Use this when the skill under test needs sibling skills available. The
    /// skill-under-test is staged under a unique slug, and every *other* skill
    /// folder inside this directory is staged under its natural name so
    /// cross-references resolve. Omit it for the default single-skill isolated
    /// run.
    #[arg(long)]
    pub skill_dir: Option<String>,
    /// Skill under evaluation.
    ///
    /// With `--skill-dir`, this is the child folder name, inferred when the
    /// directory contains exactly one skill. Without `--skill-dir`, this is a
    /// skill directory path, or a child directory name relative to the current
    /// directory. Omit it when running from inside the skill directory.
    #[arg(long)]
    pub skill: Option<String>,
    /// Iteration number for post-dispatch steps (defaults to latest existing).
    #[arg(long)]
    pub iteration: Option<u32>,
    /// Comparison mode: `new-skill` (default, with vs. without) or `revision`
    /// (old vs. new).
    ///
    /// Mode A (`new-skill`) validates a brand-new skill against baseline behavior
    /// with no skill loaded. Mode B (`revision`) tests a language change to an
    /// existing skill: snapshot the old `SKILL.md` (see `snapshot`), then run both
    /// variants against the same prompts. `revision` defaults `--baseline` to
    /// `baseline`.
    #[arg(long)]
    pub mode: Option<String>,
    /// Target harness: `claude-code` (default) or `codex`.
    ///
    /// Claude Code and Codex both support staged skills, transcript ingest, and
    /// `--guard`. Codex stages skills under `.agents/skills` and reads each
    /// task's `outputs/codex-events.jsonl` instead of a subagents dir.
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
    /// Directory whose child skills' `evals.json` files should be batch validated.
    #[arg(long)]
    pub skill_dir: Option<String>,
    /// Skill directory to validate when `--skill-dir` is omitted.
    #[arg(long)]
    pub skill: Option<String>,
}

/// `init` writes the first eval scaffold for a skill.
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Optional directory containing the skill under evaluation.
    ///
    /// Use this when the skill is an immediate child of a skills directory. If
    /// omitted, `init` uses `--skill <path-or-name>` or the current directory.
    /// `init` creates only the eval scaffold; it does not create the skill itself.
    #[arg(long)]
    pub skill_dir: Option<String>,
    /// Skill under evaluation.
    ///
    /// With `--skill-dir`, this is the child folder name, inferred when the
    /// directory contains exactly one skill. Without `--skill-dir`, this is a
    /// skill directory path, or a child directory name relative to the current
    /// directory. This value becomes the generated `skill_name`.
    #[arg(long)]
    pub skill: Option<String>,
    /// Stable kebab-case id for the first eval case.
    ///
    /// If omitted, prompts interactively. The id is used as the workspace eval
    /// directory name, so it must satisfy the eval schema's kebab-case pattern.
    #[arg(long)]
    pub id: Option<String>,
    /// User-facing prompt the eval subagent receives.
    ///
    /// If omitted, prompts interactively. Write this like a realistic user
    /// request, not like an instruction to satisfy the eval.
    #[arg(long)]
    pub prompt: Option<String>,
    /// Human-readable description of a successful response.
    ///
    /// If omitted, prompts interactively. This seeds `expected_output`; add
    /// concrete assertions after seeing iteration 1 outputs.
    #[arg(long = "expected-output")]
    pub expected_output: Option<String>,
    /// Whether the skill is expected to trigger for this eval.
    ///
    /// Defaults to true and is omitted from the generated JSON. Set false for
    /// negative evals where correct behavior is not invoking the skill.
    #[arg(long)]
    pub skill_should_trigger: Option<bool>,
    /// Overwrite an existing `<skill>/evals/evals.json`.
    ///
    /// Refuses to overwrite existing evals by default and checks that before
    /// prompting for seed fields.
    #[arg(long)]
    pub force: bool,
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
    ///
    /// Overrides a `label` recorded in the iteration's `conditions.json` (set via
    /// `run --label`); when both are absent, `BASELINE.md` shows `(none)`.
    #[arg(long)]
    pub label: Option<String>,
    /// Operator-declared agent model, recorded in `BASELINE.md`.
    ///
    /// Overrides an `agent_model` recorded in the iteration's `conditions.json`
    /// (set via `run --agent-model`); when both are absent, `BASELINE.md` shows
    /// `unspecified`.
    #[arg(long)]
    pub agent_model: Option<String>,
    /// Operator-declared judge model, recorded in `BASELINE.md`.
    ///
    /// Overrides a `judge_model` recorded in the iteration's `conditions.json`
    /// (set via `run --judge-model`); when both are absent, `BASELINE.md` shows
    /// `unspecified`.
    #[arg(long)]
    pub judge_model: Option<String>,
}

/// `run` adds the build-time flags (mode/baseline selection, staging toggles,
/// guard, plan-mode, bootstrap) on top of the common set.
#[derive(Debug, Args)]
pub struct RunArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Baseline snapshot label (defaults to `baseline` in `--mode revision`).
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
    /// Stages a harness-native `PreToolUse` hook that *blocks* subagent
    /// writes/installs outside the eval sandbox while dispatches run. Arm it
    /// unless the user opts out. The marker auto-expires after 6h and is torn down
    /// at the next run; while armed the hook fires on your own tool calls too.
    /// If it remains armed after `finalize`, `finalize` reminds you to run
    /// `teardown-guard` before editing source.
    /// Codex dispatches must include `--dangerously-bypass-hook-trust` so the
    /// vetted project-local eval hook runs. Unguarded, stray writes are only
    /// *detected* after the fact by `detect-stray-writes`, never blocked.
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
    /// dispatch, identical across arms. Opt-in, for plan-mode-relevant skills.
    /// A harness with no profile aborts. It is text the subagent reads, not a
    /// real injected mode.
    #[arg(long)]
    pub plan_mode: bool,
    /// Runs per condition cell, for variance reduction (default: 1).
    ///
    /// Dispatches every eval N times per condition, so an iteration needs
    /// `evals × 2 conditions × N` dispatches. Each run gets its own
    /// `run-<k>/` directory under the condition (own `inputs/`, `outputs/`,
    /// `run.json`, `timing.json`, `grading.json`) and a unique
    /// `agent_description` carrying an `r<k>` segment. With N=1 the layout is
    /// unchanged (artifacts sit directly in the condition directory). The
    /// benchmark's per-condition `mean`/`stddev`/`n` then reflect all runs. A
    /// per-eval `runs` field in evals.json overrides this flag for that eval.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    pub runs: u32,
    /// Operator-declared agent model, persisted into `conditions.json`.
    ///
    /// The runner never dispatches the under-test agent itself, so it cannot
    /// observe the model — declare it here while it's fresh and
    /// `promote-baseline` records it in `BASELINE.md` automatically (its own
    /// `--agent-model` flag still overrides).
    #[arg(long)]
    pub agent_model: Option<String>,
    /// Operator-declared judge model, persisted into `conditions.json`.
    ///
    /// Like `--agent-model`, but for the grading judge; surfaced in
    /// `BASELINE.md` by `promote-baseline` (its own `--judge-model` flag still
    /// overrides).
    #[arg(long)]
    pub judge_model: Option<String>,
    /// Provenance label for this run, persisted into `conditions.json`.
    ///
    /// Surfaced in `BASELINE.md` by `promote-baseline` (its own `--label` flag
    /// still overrides).
    #[arg(long)]
    pub label: Option<String>,
}

/// Every subcommand on the CLI.
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
    /// and writes `benchmark.json`. If a live guard remains armed, prints a
    /// `teardown-guard` reminder before source edits. Requires `--iteration`.
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
    /// Scaffold a first `evals/evals.json` for a skill.
    ///
    /// Creates `<skill>/evals/evals.json` with one schema-valid seed eval, then
    /// prints the next run/ingest/finalize/promote commands. Prompts
    /// interactively for any missing seed fields, and refuses to overwrite an
    /// existing eval file unless `--force` is passed. This is scaffold-only: it
    /// does not run agents, ingest transcripts, finalize, or promote results.
    Init(InitArgs),
    /// Promote a benchmark + gradings into a committed baseline.
    PromoteBaseline(PromoteBaselineArgs),
    /// Validate `evals.json` files against the bundled schemas.
    Validate(ValidateArgs),
    /// Internal PreToolUse hook entry point. Invoked by the installed write-guard
    /// hook as `eval-magic guard <marker>`, not by users; hidden from help.
    #[command(hide = true)]
    Guard {
        /// Path to the guard marker file. Defaults to
        /// `<cwd>/.claude/skills/.slow-powers-eval-guard.json`.
        marker: Option<String>,
    },
    /// Internal Codex PreToolUse hook entry point. Invoked by the installed
    /// write-guard hook as `eval-magic guard-codex <marker>`, not by users;
    /// hidden from help.
    #[command(hide = true)]
    GuardCodex {
        /// Path to the guard marker file. Defaults to
        /// `<cwd>/.agents/skills/.slow-powers-eval-guard.json`.
        marker: Option<String>,
    },
}
