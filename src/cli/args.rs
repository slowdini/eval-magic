//! The `clap` derive command tree: the top-level parser, the shared/per-command
//! argument groups, and the subcommand enum.
//!
//! Flags are intentionally permissive (mostly optional); each handler tightens
//! them to what it actually requires (see the handlers in [`super::commands`]).

use clap::{Args, Parser, Subcommand};

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
    /// Load a one-off harness descriptor file as the top registry layer.
    ///
    /// The file merges field-by-field onto any registered harness with the same
    /// `label` (or defines a new one), exactly like a project-local descriptor
    /// in `.eval-magic/harnesses/`, and — when `--harness` is omitted — its
    /// label becomes the invocation's default harness. Unlike discovered
    /// descriptor files (skipped with a warning when broken), errors in this
    /// explicitly named file are fatal.
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
    /// Target harness: `claude-code` (default), `codex`, or `opencode`.
    ///
    /// All three built-ins support staged skills, transcript ingest, scripted
    /// same-session follow-ups, and the automatically armed write guard. Each
    /// reads its own per-task events file; Codex stages skills under
    /// `.agents/skills`, and OpenCode under `.opencode/skills`.
    /// The name is resolved against the harness descriptor registry after
    /// parsing; an unknown name errors listing every registered harness.
    #[arg(long)]
    pub harness: Option<String>,
    /// Workspace directory (defaults to `<cwd>/.eval-magic`).
    ///
    /// The artifact root. Pass the same value to every command of a run, including
    /// `teardown`.
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
    /// trivial prompt in a throwaway temp dir, runs it via `/bin/sh -c` from
    /// the temp `eval_root`, and verifies `outputs/final-message.md` is
    /// recovered (non-empty). It additionally render-only-validates
    /// `parallel_command_template` and `judge_command_template` for
    /// placeholder-shape errors — rendering each with stand-in values and
    /// reporting any unresolved `{token}` the run would later surface.
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
        /// final-message recovery (opt-in; costs real CLI usage). See the
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
    /// meta-check tier and inlines only SKILL.md (not sibling skills or sibling
    /// asset files); use the staged (default) path when the measured behavior
    /// depends on sibling files. The isolated env (`env/`) is still built either
    /// way — `--no-stage` only skips populating the harness skills dir. Also
    /// disables the write guard (auto-armed or explicit) — it requires staging —
    /// so no-stage runs are unguarded and rely on `detect-stray-writes` after
    /// the fact.
    #[arg(long)]
    pub no_stage: bool,
    /// Arm the write guard explicitly (it auto-arms when the harness supports it).
    ///
    /// The write guard arms automatically on guard-capable built-in harnesses
    /// whenever staging is active, so this flag only makes the request explicit:
    /// where auto-arm quietly stays off (no declared guard, or `--no-stage`), an
    /// explicit `--guard` warns, and a harness defined only by user-supplied
    /// descriptors rejects it in preflight (rerun without it;
    /// `detect-stray-writes` audits after the fact). Opt out with `--no-guard`.
    ///
    /// The guard is a harness-native `PreToolUse` hook that *blocks* subagent
    /// writes/installs outside the isolated run env (the agent-under-test's cwd)
    /// while dispatches run. The task env is its sole allowed write root; host temp
    /// directories are out of bounds. Dispatch prompts name `<eval-root>/tmp` as
    /// the task-local scratch directory (create it when needed); eval-magic does
    /// not rewrite `TMPDIR`, `TMP`, or `TEMP`.
    /// Because the harness already cwd-bounds the agent's direct file tools to the
    /// env, the guard's main remaining value is blocking Bash-subprocess escapes the
    /// cwd boundary doesn't cover — `npm install`, `git worktree add`, `sed -i`,
    /// redirects that resolve outside the env — and acting as a backstop when the
    /// isolated session runs with relaxed permissions. Local Git operations such
    /// as status, diff, add, commit, and branching are allowed inside the task
    /// repository. Repository-routing escapes and remote Git operations are
    /// blocked; `--no-guard` opts out of those blocks, though task repositories
    /// still begin with no remotes. Literal relative redirect and `tee` targets
    /// resolve from the tool invocation cwd; dynamic, malformed, or outside
    /// targets are blocked. Every denial appends privacy-safe metadata
    /// (never the full command or patch) to the task's
    /// `.eval-magic-outputs/guard-denials.jsonl`; `ingest` joins those logs into
    /// `guard-denials.json`, and `aggregate` emits one validity warning per
    /// affected task. Re-arming truncates stale raw records; disarming the guard
    /// preserves the current log. The marker auto-expires after 6h and is torn
    /// down at the next run; while armed the
    /// hook fires on your own tool calls too. If it remains armed after `finalize`,
    /// `finalize` reminds you to run `teardown` before editing source (which disarms
    /// the cwd guard and every per-`(group, condition)` Cli env's guard). Requires
    /// staging — with `--no-stage` the guard stays off and the run is unguarded.
    /// Codex eval-agent dispatches must include
    /// `--dangerously-bypass-hook-trust` so the vetted project-local eval hook
    /// runs; judge recipes omit it because judges run outside guarded task envs.
    /// Unguarded, stray writes are only *detected* after the fact by
    /// `detect-stray-writes`, never blocked.
    /// Under Claude Code the `PreToolUse` hook is staged in each env's
    /// `.claude/settings.local.json`, and each `claude -p` dispatch loads it from
    /// that cwd (`cd <eval-root>`), enforcing the eval boundary (the recipe never
    /// passes `--bare`).
    /// When invoking this from inside Codex, staging writes `.agents/skills` and
    /// guarded runs also write `.codex/hooks.json`; Codex protects those paths in
    /// its default workspace-write sandbox, so approval/escalation may be needed.
    #[arg(long)]
    pub guard: bool,
    /// Opt out of the write guard for this run.
    ///
    /// The guard arms automatically on harnesses that declare one (see
    /// `--guard`). Pass this to run unguarded — e.g. when the skill under test
    /// legitimately writes outside the isolated run env — and to silence the
    /// unguarded-harness preflight warning. Unguarded, out-of-bounds writes are
    /// only *detected* after the fact by `detect-stray-writes` (folded into
    /// `ingest`), never blocked.
    ///
    /// Dispatches deliberately run with relaxed harness permissions so the
    /// agent-under-test can actually execute commands, which makes the guard
    /// the only enforcement boundary. Opting out therefore leaves the dispatch
    /// with no boundary at all, not merely a weaker one.
    #[arg(long, conflicts_with = "guard")]
    pub no_guard: bool,
    /// Stage the skill-under-test under this verbatim name instead of the
    /// conspicuous `slow-powers-eval-…` slug.
    ///
    /// For name-confound experiments. Single-staging-condition modes only; refuses
    /// to clobber an existing dir; registered for next-run cleanup.
    #[arg(long)]
    pub stage_name: Option<String>,
    /// Inject the shared plan-mode profile as an operating-context layer.
    ///
    /// Injects the shared, harness-agnostic plan-mode procedure
    /// (`profiles/shared/plan-mode.md`) as a `<system-reminder>` in every
    /// dispatch, identical across arms and harnesses. Opt-in, for
    /// plan-mode-relevant skills. It is text the subagent reads, not a real
    /// injected mode.
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
    /// Agent-under-test model for CLI dispatches; otherwise recorded as
    /// provenance.
    ///
    /// The run's dispatch recipes include the harness-native model flag when the
    /// adapter supports one (e.g. Codex's `-m`, Claude Code's `--model`); otherwise
    /// the value is persisted to `conditions.json` for `promote-baseline`.
    #[arg(long)]
    pub agent_model: Option<String>,
    /// Environment override for eval-agent dispatches (`KEY=VALUE`, repeatable).
    ///
    /// Descriptor defaults from `[dispatch.env]` apply first; repeated CLI
    /// entries override them by key, with the last occurrence winning. Values
    /// may be empty and may contain `=`. The resolved map is recorded in
    /// `conditions.json` and `dispatch.json`, so do not use this flag for
    /// secrets. This does not affect judge agents or runner-owned
    /// `command_check` assertions. Unset keys keep inheriting the operator's
    /// environment.
    #[arg(long, value_name = "KEY=VALUE")]
    pub agent_env: Vec<String>,
    /// Default judge model for emitted judge tasks.
    ///
    /// `grade` writes this into `judge-tasks.json` for judge tasks that do not
    /// have an assertion-level `model` override, and Cli harness judge recipes
    /// pass it through using the harness-native model flag. Also persists to
    /// `conditions.json` for `promote-baseline`.
    #[arg(long)]
    pub judge_model: Option<String>,
    /// Provenance label for this run, persisted into `conditions.json`.
    ///
    /// Surfaced in `BASELINE.md` by `promote-baseline` (its own `--label` flag
    /// still overrides).
    #[arg(long)]
    pub label: Option<String>,
}

/// Execute one runner-owned multi-turn task from a generated dispatch plan.
#[derive(Debug, Args)]
pub struct DispatchTaskArgs {
    /// Path to the runner-generated dispatch.json.
    #[arg(long, value_name = "PATH")]
    pub dispatch: String,
    /// Zero-based index into dispatch.json's tasks array.
    #[arg(long)]
    pub task_index: usize,
    /// Replace an existing conversation.json and rerun the task.
    #[arg(long)]
    pub overwrite: bool,
}

/// Every subcommand on the CLI.
#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    /// Build dispatches and run evals (the default action).
    ///
    /// Builds the iteration workspace, snapshots the `SKILL.md`, stages skills, and
    /// emits `dispatch.json` (machine-readable) alongside `dispatch-manifest.md`
    /// (human-readable). Dispatch each task through the harness CLI (`claude -p`,
    /// `codex exec`). Also writes `RUNBOOK.md`, a human-followable handoff for the
    /// run ("Read and follow RUNBOOK.md").
    ///
    /// Git is required. Every task environment is initialized as an independent,
    /// clean repository on branch `work` with a deterministic baseline commit and
    /// no remotes. The runner owns its root `.git`; task outputs remain ignored,
    /// and rebuilding an explicit iteration resets prior Git history, branches,
    /// and remotes before dispatch.
    ///
    /// Before dispatch, a shadow preflight scans every task environment for live
    /// copies of the staged skills — installed plugins, global and cross-harness
    /// skill directories — and warns when one could contaminate the comparison. It
    /// reports what is discoverable, not what a dispatch loaded: eval-magic never
    /// reads your command templates, so a remedy you applied is invisible to it.
    /// Isolating each dispatch from those sources, and confirming it worked, is
    /// `eval-magic docs isolation`.
    Run(RunArgs),
    /// Execute one scripted multi-turn task through its harness CLI.
    ///
    /// Starts the task, resumes the same native session for every delivered
    /// follow-up, and writes the task's conversation.json completion artifact.
    /// One-shot tasks continue to use the commands in dispatch-manifest.md.
    DispatchTask(DispatchTaskArgs),
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
    /// writes, and maps raw per-env guard logs through `dispatch.json` into
    /// `guard-denials.json` (including tasks without `run.json`). Malformed raw
    /// records fail with their source path and line number. It captures always-on
    /// final-environment files/lines/hunks in `diff-scope.json`, grades
    /// `transcript_check` assertions, prepares
    /// `diff_scope` grading for finalize, injects held-out
    /// `command_check.setup_files`, and executes each
    /// runner-owned command check in its task environment, applying its
    /// environment overrides and running every environment matrix cell. Diff
    /// scope is captured before held-out files are injected. Then stops at the
    /// judge hand-off, listing a judge task per `llm_judge` assertion. Requires
    /// `--iteration`; reads each task's `outputs/<harness>-events.jsonl` when the
    /// harness exposes transcripts. When the harness provides a judge recipe, it
    /// skips existing nonempty responses, prints `N/M verdicts present`, and exits
    /// nonzero while any are missing; rerun the same recipe to fill the gaps.
    /// Re-running after a fix is safe — every sub-step skips work already done.
    Ingest(CommonArgs),
    /// Finalize grading after judge responses are in.
    ///
    /// Fixed-order chain: grade `--finalize` → aggregate. Merges judge verdicts,
    /// runner-owned `command_check` results, and deterministic `diff_scope`
    /// files/lines thresholds into normal `grading.json` files, then writes
    /// `benchmark.json` with a per-assertion `passed`/`n` rollup from observed
    /// assertion results and raw per-run metrics from `diff-scope.json`. If a live
    /// guard remains armed — the cwd guard, or any per-task Cli env guard — prints
    /// a `teardown` reminder before source edits. Requires `--iteration`.
    Finalize(CommonArgs),
    /// Assemble run records from a dispatch and its transcripts.
    ///
    /// Assembles a schema-valid `run.json` and backfills `timing.json` for every
    /// task in a runner-built iteration, from `dispatch.json` +
    /// `outputs/final-message.md` + each task's `outputs/<harness>-events.jsonl`.
    /// Never clobbers existing records without `--overwrite`; transcript-derived
    /// timing carries `"source": "transcript"`. Use `--overwrite` to regenerate
    /// records and timing after extractor accounting changes. Folded into `ingest`.
    ///
    /// For harnesses whose captures identify a refused tool call (Claude Code
    /// and Codex today), it also writes `permission-denials.json` and warns on
    /// stderr: the dispatch can exit 0 either way, so a run the harness refused
    /// — and which therefore fell back to static reasoning — is otherwise
    /// invisible. `aggregate` lifts one validity warning per affected task from
    /// that file. No file is written for a harness that cannot detect a refusal,
    /// so its absence never reads as "nothing was refused".
    ///
    /// For harnesses whose captures report the session's discoverable skills and
    /// plugins (Claude Code today), it also writes `session-surface.json` — one
    /// entry per dispatch and per resumed turn — and uses it to resolve the
    /// build-time shadow preflight's findings, writing `resolved_severity` back
    /// into `plugin-shadow.json`. A finding refuted in every expected cell
    /// becomes `isolated` and raises no validity warning; refuting requires every
    /// cell to have reported, so a missing transcript leaves it unverified rather
    /// than isolated. No file is written for a harness that cannot report a
    /// surface, so its absence never reads as "nothing loaded". See
    /// `eval-magic docs isolation`.
    RecordRuns(CommonArgs),
    /// Populate tool invocations from persisted transcripts.
    ///
    /// Reads each task's `outputs/<harness>-events.jsonl` and populates
    /// `tool_invocations` in `run.json`. Subsumed by `record-runs` for
    /// runner-built iterations; still the tool for filling a pre-existing (hand- or
    /// agent-written) `run.json`.
    FillTranscripts(CommonArgs),
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
    /// Captures always-on final-environment files/lines/hunks in `diff-scope.json`
    /// and evaluates `transcript_check` assertions directly: regex against
    /// tool invocations or, for scripted evals, assistant messages across rounds.
    /// Checks can require a match before the final completion claim or before the
    /// first write/patch tool call. A `diff_scope` assertion gates the captured file count
    /// and/or added-plus-removed line count. Grade captures scope before it injects
    /// held-out `command_check.setup_files` and executes each runner-owned command
    /// in its task environment, applying fixed environment overrides and running
    /// every environment matrix cell; completed command and diff-scope results
    /// are reused. Emits judge-task files for `llm_judge` assertions; with
    /// `--finalize`, merges every result into per-run `grading.json`.
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
    /// pass-rate / duration / token stats per condition, a per-assertion
    /// `passed`/`n` rollup from observed assertion results, the delta,
    /// `validity_warnings` (including incomplete timing sample counts, one per
    /// task in `guard-denials.json`, and one per task in
    /// `permission-denials.json` whose refusals were not the guard's own, plus
    /// grouped findings in schema-v2 `plugin-shadow.json` (legacy unversioned
    /// reports remain readable) unless it records the resolved descriptor's
    /// `isolates_live_sources = true` assertion), and raw per-run files/lines/hunks
    /// from `diff-scope.json`. Shadow findings retain their intrinsic warning or
    /// comparison-invalid severity, per-cell appearances, resolution, and
    /// remediation. A timing metric with `n: 0` is unavailable, not a measured
    /// zero. The top-level `diff_scope` field is omitted for compatible older
    /// iterations that predate metric capture.
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
    Init(InitArgs),
    /// Promote a benchmark + gradings into a committed baseline.
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
    /// The user-facing reference docs ship inside the binary — version-matched
    /// to the installed release and readable offline. `guide` is the complete
    /// operating guide (the README); `byoh` is the bring-your-own-harness
    /// descriptor authoring guide; `isolation` covers isolating dispatches from
    /// live and installed skill sources, and verifying it worked. Development
    /// docs for working on eval-magic itself stay in the repository's `docs/`
    /// directory.
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
