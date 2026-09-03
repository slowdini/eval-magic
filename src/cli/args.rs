//! The `clap` derive command tree: the top-level parser, the shared/per-command
//! argument groups, and the subcommand enum.
//!
//! Flags are intentionally permissive (mostly optional); each handler tightens
//! them to what it actually requires (see the handlers in [`super::commands`]).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

pub(crate) use super::init_args::InitArgs;
pub(crate) use super::run_args::RunArgs;

/// Run skill evals — measure whether an agent skill actually shifts behavior.
///
/// An eval dispatches a fresh subagent twice per test case — once with the skill
/// loaded, once without (or old version vs. new) — and grades both outputs against
/// assertions. The pass-rate delta tells you whether the skill is worth shipping.
/// This CLI builds the workspace, stages skills for discovery, dispatches every
/// subagent through your chosen harness CLI, assembles run records from the
/// transcripts, grades, and aggregates.
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
    /// Load a one-off harness descriptor file as the top registry layer.
    ///
    /// The file merges field-by-field onto any registered harness with the same
    /// `label` (or defines a new one), exactly like a project-local descriptor
    /// in `.eval-magic/harnesses/`, and — when `--harness` is omitted — its
    /// label becomes the invocation's default harness. Unlike discovered
    /// descriptor files (skipped with a warning when broken), errors in this
    /// explicitly named file are fatal.
    ///
    /// Every command `run` generates — the printed Next: steps and the
    /// RUNBOOK.md — re-emits this flag, because the descriptor it loads can
    /// decide whether a comparison is valid (dispatch templates, shadow
    /// isolation). Follow those commands verbatim: `run` records the resolved
    /// descriptor's digest in conditions.json, and `dispatch`/`ingest` warn
    /// when a follow-up resolves a different one.
    #[arg(long, global = true, value_name = "PATH")]
    pub harness_file: Option<String>,
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
    /// cross-references resolve. The roster is read once, when the run resolves,
    /// and copied into the eval home with the skill itself; `conditions.json`
    /// records it, so what a report claims and what the environments held cannot
    /// disagree. Omit it for the default single-skill isolated run.
    #[arg(long)]
    pub skill_dir: Option<String>,
    /// Eval owner and, for a multi-skill treatment, one member of the set.
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
    /// Mode A (`new-skill`) validates the skill or ordered skill set declared by
    /// `skill_name` against baseline behavior with none of those treatment skills
    /// loaded. Mode B (`revision`) snapshots and compares every treatment member.
    /// `revision` defaults `--baseline` to `baseline`.
    #[arg(long)]
    pub mode: Option<String>,
    /// Target harness: `claude-code` (default), `cline`, `codex`, or `opencode`.
    ///
    /// All four built-ins support staged skills, transcript ingest, and runner
    /// dispatch; `claude-code`, `codex`, and `opencode` additionally support
    /// scripted same-session follow-ups and the automatically armed write guard.
    /// Each reads its own per-task events file; Claude Code stages skills under
    /// `.claude/skills`, Cline under `.cline/skills`, Codex under
    /// `.agents/skills`, and OpenCode under `.opencode/skills`.
    /// The name is resolved against the harness descriptor registry after
    /// parsing; an unknown name errors listing every registered harness.
    #[arg(long)]
    pub harness: Option<String>,
    /// Workspace directory — the eval home (defaults outside the skill's repo).
    ///
    /// The artifact root. Iterations, envs, and campaign artifacts live here, so
    /// it deliberately defaults outside the skill under test: a run never writes
    /// into the repository it is measuring. The default is
    /// `$XDG_DATA_HOME/eval-magic` (or `~/.local/share/eval-magic`) plus a
    /// directory naming the skill directory it belongs to; `EVAL_MAGIC_WORKSPACE_DIR`
    /// overrides that, and this flag overrides both. `run` prints the path it
    /// chose, and every command it suggests already carries it. Pass the same
    /// value to every command of a run, including `teardown`.
    #[arg(long)]
    pub workspace_dir: Option<String>,
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
    /// Replace existing records and rerun completed command checks.
    #[arg(long)]
    pub overwrite: bool,
}

/// `harness` groups the descriptor-inspection subcommands.
#[derive(Debug, Args)]
pub struct HarnessArgs {
    #[command(subcommand)]
    pub command: HarnessCommands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum HarnessCommands {
    /// Scaffold a commented descriptor template and notes skeleton for a new
    /// harness.
    ///
    /// Writes two files into the project-local descriptor layer:
    /// `.eval-magic/harnesses/<name>.toml`, pre-filled with `label = "<name>"`
    /// and every optional table present as a commented-out, inline-explained
    /// example; and `.eval-magic/harnesses/<name>-notes.md`, the
    /// verification-record skeleton where each filled-in value's source is
    /// recorded. The scaffold is lint-clean exactly as written — `harness
    /// lint` passes before a single field is filled in, and the file registers
    /// `<name>` as a baseline harness immediately. Values must be verified
    /// against the harness's own documentation or observed output, never
    /// guessed. Scaffolding an already-registered name (e.g. the built-in
    /// `claude-code`) is allowed with a note: descriptor layers merge
    /// field-by-field, so the file overlays the registered harness instead of
    /// defining a new one. The `[guard]` table is never scaffolded —
    /// user-supplied descriptors may not declare it. The authoring guide is
    /// `eval-magic docs byoh`; its "Upstreaming your descriptor" section covers
    /// contributing the finished descriptor.
    Init {
        /// Label for the new harness (kebab-case, e.g. `cool-cli`); becomes
        /// the descriptor's `label` and both file names.
        name: String,
        /// Print the rendered descriptor template to stdout instead of
        /// writing files.
        ///
        /// Prints only the template (no notes skeleton, no next steps), so
        /// the output is redirectable — e.g. into a user-global layer
        /// (`~/.config/eval-magic/harnesses/`) or a one-off `--harness-file`.
        #[arg(long)]
        stdout: bool,
        /// Overwrite existing scaffold files.
        #[arg(long)]
        force: bool,
    },
    /// List every registered harness: name, source layer(s), enhancements.
    ///
    /// One line per harness with the layers that contributed to it (built-in,
    /// user, project, file — a merged descriptor shows all of them, e.g.
    /// `built-in + project`) and the enhancements the resolved descriptor
    /// declares (staging, skills-block, transcript, model-flag, guard,
    /// shadow-preflight, dispatch-recipes, conversation-resume; `baseline` when
    /// none). The session's
    /// default harness — `claude-code`, or the `--harness-file` descriptor when
    /// one is loaded — is marked `(default)`.
    List,
    /// Print one harness's resolved descriptor (after layer merging) as TOML.
    ///
    /// The output is authorable: the provenance header lists every contributing
    /// file as `#` comments, and the body is valid descriptor TOML you can copy
    /// into a layer file (`.eval-magic/harnesses/<name>.toml`) and edit. Fields
    /// at their baseline defaults are omitted. When reusing a guarded built-in's
    /// output as a user layer, drop its `[guard]` table and
    /// `run.supports_guard` first — user-supplied descriptors may not declare
    /// them.
    Show {
        /// Registered harness name (see `harness list`).
        name: String,
    },
    /// Validate a descriptor file, or every layer of a registered harness.
    ///
    /// Descriptor file targets are linted as user-supplied by default and run
    /// the full load pipeline with one ✓/✗ line per check: TOML syntax + schema
    /// (unknown fields, bad capability names), the user-layer restrictions
    /// (`[guard]` and `run.supports_guard = true` stay built-in-only; unguarded
    /// runs fall back to the detect-stray-writes audit), and the cross-field
    /// invariants — merged onto the registered harness with the same `label`
    /// when one exists, so a partial override is checked against its real merge
    /// target. A name target re-lints every discovered layer file strictly,
    /// preserving each source's actual layer and surfacing descriptors that
    /// registry initialization skipped with a warning. Exits non-zero when any
    /// check fails.
    ///
    /// For eval-magic developers checking an on-disk built-in source before a
    /// rebuild, `--as-builtin` skips only the user-layer restriction. It requires
    /// a positional file target, cannot be combined with `--harness-file`, and
    /// does not change registry loading; user-supplied descriptors remain unable
    /// to register built-in-only guard data.
    ///
    /// With `--probe`, and only after every static check passes, also exercises
    /// the descriptor end-to-end: renders `dispatch.exec_template` with a
    /// trivial prompt in a throwaway temp dir, runs it via `sh -c` from
    /// the temp `eval_root`, parses the configured transcript capture, and
    /// verifies it contains a non-empty final response.
    ///
    /// `--probe` invokes the real harness CLI (network, tokens, usage
    /// limits), so it is opt-in and never runs as part of standard CI checks
    /// — it is a one-time BYOH check. Before exec it prints an "about to
    /// execute" banner with the rendered command and asks `y/N`; pass `--yes`
    /// to skip the prompt, and `--probe-timeout <SECS>` (default 300 = 5 min)
    /// to cap the run.
    Lint {
        /// Descriptor file path, or a registered harness name.
        ///
        /// Optional when `--harness-file` is passed: that file is then the
        /// target, since it already names exactly one descriptor.
        target: Option<String>,
        /// Treat a descriptor file as a built-in source for this lint only.
        ///
        /// Skips the user-layer restriction so eval-magic developers can check
        /// an edited source under `harnesses/` without rebuilding first. Schema
        /// and cross-field invariant checks still run. This does not change
        /// registry loading, requires a positional file target, and cannot be
        /// combined with `--harness-file`.
        #[arg(long, requires = "target")]
        as_builtin: bool,
        /// Execute the dispatch exec template with a trivial prompt and verify
        /// transcript-owned final-response recovery (opt-in; costs real CLI usage). See the
        /// subcommand description above for the full contract.
        #[arg(long)]
        probe: bool,
        /// Skip the interactive `y/N` confirm before a `--probe` exec.
        /// Default-deny when stdin is not a TTY, so the probe never runs
        /// unattended inside a pipe or CI step.
        #[arg(long)]
        yes: bool,
        /// Override the `--probe` exec timeout in seconds (default 300 = 5 min).
        #[arg(long, value_name = "SECS")]
        probe_timeout: Option<u64>,
    },
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
    /// Operator-declared responder model, recorded in `BASELINE.md`.
    ///
    /// Overrides a `responder_model` recorded in the iteration's
    /// `conditions.json` (set via `run --responder-model`); when both are
    /// absent, `BASELINE.md` shows `unspecified`.
    #[arg(long)]
    pub responder_model: Option<String>,
}

#[derive(Debug, Args)]
pub struct DispatchArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Dispatch only these zero-based `tasks[]` indices; repeatable. Every task
    /// in the plan by default.
    #[arg(long = "task-index")]
    pub task_index: Vec<usize>,
    /// Kill a task that runs longer than this many seconds and record it as
    /// timed out, so one hung dispatch cannot stall the campaign. `0` disables
    /// the deadline entirely.
    #[arg(long, default_value_t = 1800, value_name = "SECONDS")]
    pub timeout: u64,
    /// How many tasks to dispatch at once. Each task owns a private environment,
    /// so they are independent.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u32).range(1..))]
    pub jobs: u32,
    /// Dispatch the judge tasks `ingest` emitted instead of the eval tasks.
    /// Skips existing nonempty responses, prints `N/M verdicts present`, and
    /// exits nonzero while any are missing; rerun to fill the gaps.
    #[arg(long)]
    pub judges: bool,
}

/// Every subcommand on the CLI.
#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    /// Build dispatches and run evals (the default action).
    ///
    /// Builds the iteration workspace, copies and stages the treatment skill set, and
    /// emits `dispatch.json` (machine-readable) alongside `dispatch-manifest.md`
    /// (human-readable). It prepares the run but does not dispatch agents —
    /// `eval-magic dispatch` does. After setup, read `RUNBOOK.md` end to end; that
    /// generated file is the authority for the dispatch, ingest, judge, finalize,
    /// and teardown commands for the selected harness.
    ///
    /// A case with effective run count `R` creates `2R` native agent sessions: one
    /// per condition and repetition. Scripted follow-ups add up to `2R × F` model
    /// turns for `F` declared follow-ups. A `responder` case instead adds one
    /// agent turn and one small responder dispatch per round, up to its
    /// `max_turns` bound. Each `llm_judge` assertion creates its effective sample
    /// count of judge tasks per condition and repetition. Review the printed run
    /// summary and obtain confirmation before spending model usage.
    ///
    /// Git is required. Every task environment is initialized as an independent,
    /// clean repository on branch `work` with a deterministic baseline commit and
    /// no remotes. The runner owns its root `.git`; task outputs remain ignored,
    /// and rebuilding an explicit iteration resets prior Git history, branches,
    /// and remotes before dispatch.
    ///
    /// Before dispatch, a shadow preflight scans every task environment for other
    /// discoverable copies of evaluated skills. Schema-v3 `plugin-shadow.json`
    /// distinguishes operator-environment sources (installed plugins, global and
    /// cross-harness directories) from project skills preserved from a sourced
    /// codebase. A codebase keeps its instructions, harness config, and project
    /// skills by default; set its `exclude_skill_sources` field to remove only the
    /// selected harness's project skill roots symmetrically before staging. See
    /// `eval-magic docs codebase` for configuration and provenance, and
    /// `eval-magic docs isolation` for operator-source remedies and verification.
    ///
    /// Staged skills, guard files, and framework outputs sit inside the task
    /// repository, so every environment also gets the project's own ignore files
    /// (`.prettierignore` and friends, detected from the codebase's tooling)
    /// taught to skip them — identically in both arms, so a project lint step
    /// cannot fail in the treatment arm alone. Set the codebase's `ignore_files`
    /// to name those files yourself, or to `[]` to opt out. See
    /// `eval-magic docs codebase`.
    Run(RunArgs),
    /// Run every task in a prepared iteration through its harness CLI.
    ///
    /// Reads `dispatch.json`, executes each task in its own private environment,
    /// and writes the task's `conversation.json` completion artifact. A task that
    /// already has one is skipped, so rerunning after a failure retries only what
    /// did not finish; `--overwrite` redispatches regardless.
    ///
    /// A task that fails is recorded and the batch continues — one bad dispatch
    /// does not abandon the campaign. The command exits nonzero if any task
    /// failed. A conversation that stops at a scripted gate is valid eval data,
    /// not a failure.
    ///
    /// A multi-turn task — one declaring scripted `turns`, or a `responder` that
    /// derives them — resumes the same native session for every turn it
    /// delivers, and each round must report the same native session ID or that
    /// task fails. A completed or normally stopped conversation records
    /// `delivered_followups`; an interrupted task commits no artifact, so a
    /// rerun picks it up.
    ///
    /// A responder task adds one small consultation after every round, run
    /// through the same harness on `run --responder-model` and captured under
    /// the run's `responder/` directory. A responder that produced no usable
    /// reply, or that hit its `max_turns` bound, is recorded and warned about by
    /// cause: the run ended mid-task, so read its last assistant message before
    /// trusting it. See `eval-magic docs conversations`.
    Dispatch(DispatchArgs),
    /// Snapshot a workspace baseline.
    ///
    /// Snapshots the treatment as a Mode B baseline under
    /// `<workspace>/<eval-owner>/snapshots/<label>/`. A list-authored treatment
    /// snapshots every member atomically. Snapshots persist across iterations;
    /// delete them by hand when no longer needed.
    Snapshot(SnapshotArgs),
    /// Tear down a workspace.
    ///
    /// Disarms the guard, removes the staged skill set, and reclaims the workspace
    /// artifacts that are safe to delete. Run it at the end of a run.
    Teardown(CommonArgs),
    /// Disarm the write guard.
    ///
    /// Removes only the write guard (e.g. mid-run, before hand-editing files the
    /// guard would block) — at the invocation cwd, and in every
    /// per-`(group, condition)` env of the iteration the shared target flags
    /// select (`--skill-dir`/`--skill`, `--workspace-dir`, `--iteration`;
    /// `--iteration` defaults to the latest). Running it from inside a task env
    /// needs no flags: that env is the cwd. Where those flags resolve no run, it
    /// sweeps the cwd alone and says which guards it could not check, rather
    /// than reporting an all-clear for them. The full `teardown` removes the
    /// guard AND the staged skill set, and reclaims the workspace.
    TeardownGuard(CommonArgs),
    /// Ingest recorded transcripts into run records.
    ///
    /// Fixed-order chain: record-runs → detect-stray-writes → grade. Assembles
    /// each task's `run.json` + `timing.json`, scans for stray
    /// writes, and maps raw per-env guard logs through `dispatch.json` into
    /// `guard-denials.json` (including tasks without `run.json`). Malformed raw
    /// records fail with their source path and line number. It measures the
    /// finished environment against the `eval-magic/baseline` ref it was marked
    /// with, writing always-on files/lines/hunks and the changed-file list to
    /// `diff-scope.json` and the diff itself to `diff.patch`, grades
    /// `transcript_check` assertions (tool patterns match native names plus the
    /// portable spellings the descriptors declare for that tool's role), prepares
    /// `diff_scope` grading for finalize, injects held-out
    /// `command_check.setup_files`, and executes each
    /// runner-owned command check in its task environment, applying its
    /// environment overrides and running every environment matrix cell. A task
    /// is eligible only after its runner-owned `run.json` exists, so a partial
    /// ingest leaves incomplete task environments untouched. A cached result is
    /// reused only when its authored definition and run-record digests match;
    /// legacy or stale results are executed again. Diff scope is captured before
    /// held-out files are injected. Then stops at the
    /// judge hand-off, writing one bounded `judge-evidence.md` per recorded run
    /// and listing the effective sample count of judge tasks per `llm_judge`
    /// assertion. The exact evidence bundle is shared by that run's tasks and
    /// inlined into their prompts. Requires `--iteration`; reads each task's
    /// `outputs/<harness>-events.jsonl` when the harness exposes transcripts,
    /// under `outputs/turn-<n>/`. Dispatch the judge tasks it lists with
    /// `eval-magic dispatch --judges`.
    /// Re-running after a fix is safe — every sub-step skips work already done.
    Ingest(CommonArgs),
    #[command(about = super::compare_args::ABOUT, long_about = super::compare_args::LONG_ABOUT)]
    Compare(super::compare_args::CompareArgs),
    /// Finalize grading after judge responses are in.
    ///
    /// Fixed-order chain: grade `--finalize` → aggregate. Merges judge verdicts,
    /// runner-owned `command_check` results, and deterministic `diff_scope`
    /// files/lines thresholds into normal `grading.json` files, then writes
    /// `benchmark.json` with per-assertion rollups from observed assertion
    /// results. Binary assertions keep their `passed`/`n` rollup; sampled LLM
    /// assertions retain every verdict, pooled vote counts, vote proportion, and
    /// pass^k. Raw per-run metrics come from `diff-scope.json`.
    /// The per-run changed-file list and `diff.patch` stay beside each run rather
    /// than being rolled up. If a live
    /// guard remains armed — the cwd guard, or any per-task Cli env guard — prints
    /// a `teardown` reminder before source edits. Requires `--iteration`.
    Finalize(CommonArgs),
    /// Assemble run records from a dispatch and its transcripts.
    ///
    /// Assembles a schema-valid `run.json` and backfills `timing.json` for every
    /// task in a runner-built iteration, from `dispatch.json`,
    /// `conversation.json`, and each task's
    /// `outputs/turn-<n>/<harness>-events.jsonl`.
    /// Never clobbers existing records without `--overwrite`; transcript-derived
    /// timing carries `"source": "transcript"`. Use `--overwrite` to regenerate
    /// records and timing after extractor accounting changes. Folded into `ingest`.
    ///
    /// For harnesses whose captures identify a refused tool call, it also writes
    /// `permission-denials.json` and warns on
    /// stderr: the dispatch can exit 0 either way, so a run the harness refused
    /// — and which therefore fell back to static reasoning — is otherwise
    /// invisible. `aggregate` lifts one validity warning per affected task from
    /// that file. No file is written for a harness that cannot detect a refusal,
    /// so its absence never reads as "nothing was refused".
    ///
    /// For harnesses whose captures report the session's discoverable skills and
    /// plugins, it also writes `session-surface.json` — one
    /// entry per dispatch and per resumed turn — and uses it to resolve the
    /// build-time shadow preflight's findings, writing `resolved_severity` back
    /// into `plugin-shadow.json`. A finding refuted in every expected cell
    /// becomes `isolated` and raises no validity warning; refuting requires every
    /// cell to have reported, so a missing transcript leaves it unverified rather
    /// than isolated. No file is written for a harness that cannot report a
    /// surface, so its absence never reads as "nothing loaded". See
    /// `eval-magic docs isolation`.
    RecordRuns(CommonArgs),
    /// Detect writes outside each private task environment.
    ///
    /// Scans each run's `tool_invocations` and writes `stray-writes.json`: write
    /// tools targeting paths outside the run's `eval_root` (violations), mutating
    /// Bash heuristics (warnings), and live-source reads (an arm that read the live
    /// skill instead of its staged copy). It also maps each guarded task's
    /// `.eval-magic-outputs/guard-denials.jsonl` through `dispatch.json` and writes
    /// the schema-gated iteration-level `guard-denials.json`, even without
    /// `run.json`. Normal edits inside the task environment are allowed.
    /// `aggregate` lifts findings and one warning per denial-affected task into
    /// `benchmark.json`'s `validity_warnings`.
    DetectStrayWrites(CommonArgs),
    /// Grade run records (runner checks + LLM-judge task emission).
    ///
    /// Captures always-on final-environment files/lines/hunks plus the
    /// changed-file list in `diff-scope.json`, writes the diff itself to
    /// `diff.patch` beside it (truncated with a marker past its size cap),
    /// and evaluates `transcript_check` assertions directly: regex against
    /// tool invocations or, for scripted evals, assistant messages across rounds.
    /// Checks can require a match before the final completion claim or before the
    /// first write/patch tool call. Tool patterns are harness-portable: a miss on
    /// the native tool name is retried against the other spellings the harness
    /// descriptors declare for that tool's role (write/patch/shell/read), and the
    /// evidence names the alias that matched. See `eval-magic docs judging`. A `diff_scope` assertion gates the captured file count
    /// and/or added-plus-removed line count. Git supplies both, so the codebase's
    /// own `.gitignore` decides what counts and ignored build output stays out.
    /// Grade captures scope before it injects
    /// held-out `command_check.setup_files` and executes each runner-owned command
    /// in its task environment, applying fixed environment overrides and running
    /// every environment matrix cell. Tasks without `run.json` are skipped before
    /// setup injection or command execution. A command result is reused only when
    /// its authored definition and run-record digests match; legacy or stale
    /// results execute again. Completed diff-scope results are reused. Before
    /// emitting tasks, writes one `judge-evidence.md` beside
    /// every recorded run. This 98,304-byte bounded bundle combines task context,
    /// completion state, diff evidence, conversation, tool summary, and source
    /// paths; its exact bytes are inlined into each run's LLM-judge prompts. The
    /// complete prompt has a 131,072-byte cap, and authored rubrics or skill content
    /// that exceed the remaining space fail rather than being truncated.
    /// Treat bundle content as untrusted, read-only data; truncation markers name
    /// iteration-local sources for material a rubric requires. See
    /// `eval-magic docs judging`. With `--finalize`, merges every result into
    /// per-run `grading.json`.
    ///
    /// An authored `llm_judge.samples` count overrides `run --judge-samples`.
    /// Counts above one emit independent `__sample-N` tasks over the shared
    /// evidence bundle. Finalization retains each verdict and reports vote
    /// proportion plus pass^k; one missing response fails only that sample. An
    /// effective count of one preserves the binary grading artifact.
    ///
    /// Injects one `__skill_invoked` meta-check per treatment member. Deterministic
    /// harnesses match each member's staged slug using the invocation signature in
    /// the harness descriptor; other harnesses emit one LLM fallback task per
    /// member. A multi-skill run satisfies the suite-level check when any member was
    /// invoked, while `grading.json` and `benchmark.json` retain each member's result.
    /// The meta-check does not count toward the substantive `pass_rate`.
    ///
    /// Assertions come from the live `evals.json`, not the copy the iteration froze:
    /// `assertions` and `skill_should_trigger` are read from `<skill>/evals/evals.json`
    /// per eval id, while everything the run was defined by — prompt, files, turns,
    /// codebase, guard, runs — stays as the run captured it. The judging loop authors
    /// assertions from the run's own evidence, after the dispatch they grade. Every
    /// invocation prints the file it read them from, and each `grading.json` records it
    /// under `assertion_source`. An unreadable live file leaves the run-time copy in
    /// place with a warning; an invalid one stops grading. Cached judge verdicts are
    /// keyed by assertion id, so `dispatch --judges --overwrite` re-judges an edited
    /// assertion. Command checks invalidate automatically when their definition or
    /// run record changes; `grade --overwrite` also re-executes an exact cache match.
    /// See `eval-magic docs judging`.
    Grade(GradeArgs),
    /// Aggregate before/after benchmark deltas.
    ///
    /// Reads grading + timing from an iteration and writes `benchmark.json` with
    /// grading / duration / token stats per condition, a per-assertion binary
    /// `passed`/`n` or sampled-vote rollup from observed assertion results, the
    /// delta,
    /// `validity_warnings` (including incomplete timing sample counts, one per
    /// task in `guard-denials.json`, and one per task in
    /// `permission-denials.json` whose refusals were not the guard's own, plus
    /// grouped findings in schema-v3 `plugin-shadow.json` (v2 and legacy
    /// unversioned reports remain readable). Findings distinguish
    /// `operator-environment` sources from `codebase-sourced` project skills.
    /// The resolved descriptor's `isolates_live_sources = true` assertion
    /// suppresses only operator-environment findings; codebase findings require
    /// the eval's separate `codebase.exclude_skill_sources` policy. The benchmark
    /// also carries raw per-run files/lines/hunks from `diff-scope.json`. Each run's
    /// changed-file list and its `diff.patch` stay in the run directory. Shadow
    /// findings retain their intrinsic warning or comparison-invalid severity,
    /// per-cell appearances, resolution, and remediation. A timing metric with
    /// `n: 0` is unavailable, not a measured zero. The top-level `diff_scope` field
    /// is omitted for compatible older iterations that predate metric capture.
    ///
    /// Read `validity_warnings` before trusting the delta. Raw `diff_scope` entries
    /// are diagnostic context rather than an optimization target: smaller is not
    /// necessarily better. In rendered JSON, `n: 0` means unavailable, never a
    /// measured zero.
    ///
    /// Isolating dispatches from the live sources a shadow finding names, and
    /// confirming it worked, is `eval-magic docs isolation`.
    Aggregate(CommonArgs),
    /// Scaffold a first `evals/evals.json` for a skill.
    ///
    /// Creates `<skill>/evals/evals.json` with one schema-valid seed eval, then
    /// prints the next run/ingest/finalize/promote commands. Prompts
    /// interactively for any missing seed fields, and refuses to overwrite an
    /// existing eval file unless `--force` is passed. This is scaffold-only: it
    /// does not run agents, ingest transcripts, finalize, or promote results.
    ///
    /// With no codebase option, the scaffold uses eval-magic's default example
    /// codebase at a pinned commit. Choose an explicit remote with
    /// `--codebase-url` plus `--codebase-ref`, a local directory with
    /// `--codebase-path`, or the invocation directory with `--codebase-cwd`. See
    /// `eval-magic docs codebase` for source and reproducibility details.
    ///
    /// Extend the seed in `evals/evals.json`: `turns` scripts same-session
    /// follow-ups and `responder` derives them instead (see
    /// `eval-magic docs conversations`), `files_root` resolves overlay sources
    /// applied at the codebase root, and a per-eval `runs` value overrides
    /// `run --runs`. Add
    /// assertions after the first iteration, then check the file with
    /// `eval-magic validate`.
    Init(InitArgs),
    /// Promote a benchmark, gradings, and judge evidence into a committed baseline.
    ///
    /// Copies the iteration's `benchmark.json`, per-run `grading.json`, and exact
    /// bounded `judge-evidence.md` bundles to `<skill>/evals/baseline/`. The
    /// benchmark stays at that directory's root, gradings land under `grading/`,
    /// evidence bundles land under `evidence/`, and `BASELINE.md` records
    /// provenance. Missing bundles from compatible legacy iterations warn without
    /// blocking promotion. An existing hand-authored `NOTES.md` is retained; one
    /// is scaffolded when absent. Promote before teardown when the result is worth
    /// keeping. See `eval-magic docs judging` for the evidence contract.
    PromoteBaseline(PromoteBaselineArgs),
    /// Validate `evals.json` files against the bundled schemas.
    Validate(ValidateArgs),
    /// Inspect and validate harness descriptors (built-in and user-supplied).
    ///
    /// Harnesses are described by layered TOML descriptor files: embedded
    /// built-ins → user-global (`$EVAL_MAGIC_CONFIG_DIR`,
    /// `$XDG_CONFIG_HOME/eval-magic`, or `~/.config/eval-magic`, each under
    /// `harnesses/*.toml`) → project-local (`.eval-magic/harnesses/*.toml`) →
    /// a one-off `--harness-file <path>`. A later file whose `label` matches an
    /// earlier descriptor overrides individual fields (field-level merge, not
    /// whole-file shadowing); a new `label` defines a new harness usable with
    /// `--harness`. `list` surveys the registry, `show` prints one resolved
    /// descriptor, and `lint` validates a descriptor file or registered name.
    Harness(HarnessArgs),
    /// Print an embedded reference doc, or list the available topics.
    ///
    /// Every Markdown file directly under `docs/guides/` ships inside the binary,
    /// version-matched to the installed release and readable offline. `byoh`
    /// covers adapting an unknown harness; `isolation` covers excluding live
    /// skill sources after a shadow warning. Bare `docs` lists the discovered
    /// topics. Contributor documentation stays unembedded in the repository's
    /// `docs/` root.
    Docs {
        /// Topic to print (bare `docs` lists the available topics).
        topic: Option<String>,
    },
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
    /// Internal test fixture. A predictable child process for the suite to
    /// spawn — one that exits with a chosen code, emits chosen bytes, or writes
    /// a chosen file — so tests do not depend on the output conventions of
    /// utilities such as `true` or `printf`. Not for users; hidden from help.
    #[command(hide = true, name = "__fixture")]
    Fixture(FixtureArgs),
    /// Internal generic PreToolUse hook entry point. Invoked by the installed
    /// write-guard hook as `eval-magic guard-hook --harness <name> <marker>`,
    /// not by users; hidden from help. `guard` / `guard-codex` are frozen
    /// aliases of this for the claude-code and codex harnesses.
    #[command(hide = true, name = "guard-hook")]
    GuardHook {
        /// Harness whose embedded descriptor supplies the verdict shape; an
        /// unknown name fails open (allows the call).
        #[arg(long)]
        harness: String,
        /// Path to the guard marker file. Defaults to the harness's
        /// `<skills_dir>/.slow-powers-eval-guard.json` under the cwd.
        marker: Option<String>,
    },
}

/// Flags for the hidden `__fixture` subcommand.
///
/// Each flag stands in for a POSIX construct a test would otherwise spawn.
/// The fixture emits one output string built from `--pad`, then `--text`, then
/// `--echo-env` (in that order, joined by `--separator`), sends it to stdout and
/// optionally to `--write`/`--append`, and exits with `--exit` — or `1` when any
/// `--require-*` check failed.
///
/// Requirements never suppress the effects. The matrix suites read their append
/// log to prove every cell ran, including the cells expected to fail.
#[derive(Debug, Args, Default)]
pub struct FixtureArgs {
    /// Exit code to leave with when every requirement holds.
    #[arg(long, default_value_t = 0)]
    pub exit: i32,
    /// Literal output fragment; repeatable.
    #[arg(long)]
    pub text: Vec<String>,
    /// Emit the value of this environment variable; repeatable.
    #[arg(long = "echo-env")]
    pub echo_env: Vec<String>,
    /// Value emitted by `--echo-env` for a variable that is not set. Without it
    /// an unset variable contributes an empty fragment.
    #[arg(long)]
    pub default: Option<String>,
    /// Emit this many `x` bytes ahead of the other fragments, for exercising
    /// output larger than the diagnostic truncation limit.
    #[arg(long)]
    pub pad: Option<usize>,
    /// Sleep this many milliseconds before doing anything else, so a caller can
    /// overrun a deadline without depending on an external `sleep` binary.
    #[arg(long = "sleep-ms")]
    pub sleep_ms: Option<u64>,
    /// Joins the fragments. Empty by default.
    #[arg(long, default_value = "")]
    pub separator: String,
    /// Terminate the emitted output with a newline.
    #[arg(long)]
    pub newline: bool,
    /// Write this text to stderr.
    #[arg(long = "stderr")]
    pub stderr: Option<String>,
    /// Also write the emitted output to this path, replacing it.
    #[arg(long)]
    pub write: Option<PathBuf>,
    /// Also append the emitted output to this path.
    #[arg(long)]
    pub append: Option<PathBuf>,
    /// Fail unless this path exists; repeatable.
    #[arg(long = "require-file")]
    pub require_file: Vec<PathBuf>,
    /// Fail unless `<path>` holds exactly `<text>`.
    #[arg(long = "require-file-text", num_args = 2, value_names = ["PATH", "TEXT"])]
    pub require_file_text: Vec<String>,
    /// Fail unless the variable is set (`NAME`) or holds a value (`NAME=VALUE`);
    /// repeatable.
    #[arg(long = "require-env")]
    pub require_env: Vec<String>,
    /// Fail unless the two paths hold identical bytes.
    #[arg(long = "files-equal", num_args = 2, value_names = ["LEFT", "RIGHT"])]
    pub files_equal: Option<Vec<PathBuf>>,
}
