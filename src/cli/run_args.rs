//! Arguments and help text for preparing an eval run.

use clap::Args;

use super::args::CommonArgs;

/// `run` adds the build-time flags (mode/baseline selection, staging toggles,
/// guard, plan-mode, bootstrap) on top of the common set.
#[derive(Debug, Args)]
pub(crate) struct RunArgs {
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
    /// The resolved state is written as `guard_armed` in `conditions.json`,
    /// mirrored as `guard` in `dispatch.json` and `guard_armed` in
    /// `benchmark.json`, and included in promoted baseline provenance.
    ///
    /// The guard is a harness-native `PreToolUse` hook that *blocks* subagent
    /// writes/installs outside the isolated run env (the agent-under-test's cwd)
    /// while dispatches run. The task env is its sole allowed write root; host temp
    /// directories are out of bounds. Dispatch prompts name `<eval-root>/tmp` as
    /// the task-local scratch directory (create it when needed); eval-magic does
    /// not rewrite `TMPDIR`, `TMP`, or `TEMP`. Because the framework designates
    /// that directory, what an agent puts there is excluded from diff scope and
    /// from the project's own ignore files — so a `diff_scope` budget covers the
    /// change, not the scratch work, and judges never read scratch notes as the
    /// deliverable.
    /// Because the harness already cwd-bounds the agent's direct file tools to the
    /// env, the guard's main remaining value is blocking Bash-subprocess escapes the
    /// cwd boundary doesn't cover and acting as a backstop when the isolated session
    /// runs with relaxed permissions. Recognized development mutations require an
    /// allowance from the eval's `guard` configuration. `allow_commands` grants
    /// literal shell-token prefixes; `allow_tools` grants every invocation of an
    /// executable basename. A per-eval block replaces the config-level default. With
    /// no explicit block, eval-magic composes packaged profiles detected from the
    /// staged task tree. See `eval-magic docs guard` for configuration, matching,
    /// packaged profiles, and examples.
    ///
    /// Command allowances never bypass containment checks. Known destination options
    /// with dynamic, missing, or outside values are blocked, as are global/user
    /// install modes that do not have a supported in-env destination. Recognized
    /// destinations include npm `--prefix`, pnpm `-C`/`--dir`, Yarn/Bun `--cwd`,
    /// pip `--target`/`--prefix`/`--root`/`--src`, and Cargo `-C`/`--target-dir`
    /// plus its target-dir environment variables. Generic shell commands are not a
    /// complete parser: for example, a bare `touch /outside` remains an
    /// after-the-fact `detect-stray-writes` concern.
    ///
    /// Local Git operations such as status, diff, add, commit, and branching are
    /// allowed inside the task repository. Repository-routing escapes and remote Git
    /// operations are blocked; `--no-guard` opts out of those blocks, though task
    /// repositories still begin with no remotes. Literal relative redirect and `tee`
    /// targets resolve from the tool invocation cwd; dynamic, malformed, or outside
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
    /// runs; judge dispatches omit it because judges run outside guarded task envs.
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
    /// For name-confound experiments. A scalar `skill_name` and one staging
    /// condition are required; a multi-skill treatment is rejected because one
    /// override cannot name every member. Refuses to clobber an existing dir and
    /// registers the staged name for next-run cleanup.
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
    /// Before staging, a fully binary run summary prints the minimum attainable
    /// two-sided Fisher exact p-value for each effective run count, assuming
    /// perfect separation between the two conditions. A run with sampled LLM
    /// assertions instead identifies vote proportion and pass^k as non-binary
    /// endpoints. eval-magic does not calculate observed p-values or apply a
    /// significance threshold.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    pub runs: u32,
    /// Agent-under-test model for CLI dispatches; otherwise recorded as
    /// provenance.
    ///
    /// The commands `dispatch` spawns include the harness-native model flag when
    /// the adapter supports one (e.g. Codex's `-m`, Claude Code's `--model`);
    /// otherwise the value is persisted to `conditions.json` for
    /// `promote-baseline`.
    #[arg(long)]
    pub agent_model: Option<String>,
    /// Environment override for eval-agent dispatches (`KEY=VALUE`, repeatable).
    ///
    /// Descriptor defaults from `[dispatch.env]` apply first; repeated CLI
    /// entries override them by key, with the last occurrence winning. Values
    /// may be empty and may contain `=`. The resolved map is recorded in
    /// `conditions.json` and `dispatch.json`, so do not use this flag for
    /// secrets. Runner-owned `command_check` assertions are unaffected. Unset
    /// keys keep inheriting the operator's environment.
    #[arg(long, value_name = "KEY=VALUE")]
    pub agent_env: Vec<String>,
    /// Default judge model for emitted judge tasks.
    ///
    /// `grade` writes this into `judge-tasks.json` for judge tasks that do not
    /// have an assertion-level `model` override, and `dispatch --judges` passes
    /// it through using the harness-native model flag. Also persists to
    /// `conditions.json` for `promote-baseline`.
    #[arg(long)]
    pub judge_model: Option<String>,
    /// Default verdict count for authored LLM-judge assertions (default: 1).
    ///
    /// An assertion-level `samples` value overrides this option. Counts above one
    /// dispatch independent judges over the same bounded evidence bundle and are
    /// reported as vote proportion p plus pass^k = p^N. The framework-injected
    /// skill-invocation meta-check remains single-shot. See
    /// `eval-magic docs judging`.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    pub judge_samples: u32,
    /// Model that answers the agent for evals declaring a `responder`.
    ///
    /// `dispatch` consults it once after every round, through the same harness
    /// as the agent under test, using the harness-native model flag. It is
    /// run-level on purpose: answering one eval with a different model than its
    /// neighbours puts a second uncontrolled variable inside the comparison.
    /// Omit it to answer on the harness's default model. Also persists to
    /// `conditions.json` for `promote-baseline`. See
    /// `eval-magic docs conversations`.
    #[arg(long)]
    pub responder_model: Option<String>,
    /// Provenance label for this run, persisted into `conditions.json`.
    ///
    /// Surfaced in `BASELINE.md` by `promote-baseline` (its own `--label` flag
    /// still overrides).
    #[arg(long)]
    pub label: Option<String>,
}
