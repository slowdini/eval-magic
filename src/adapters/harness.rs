//! The harness adapter API — the single seam between generic dispatch code and
//! harness-specific behavior.
//!
//! The trait is tiered into a **runner requirement** every selected harness must
//! satisfy and **enhancements** that raise fidelity when native support exists:
//!
//! - **Runner requirement:** the adapter identifies itself and exposes a
//!   dispatch command plus a transcript event filename/reader. `run` rejects a
//!   selected harness missing either execution or transcript recovery.
//! - **Enhancements (defaulted):** native staging, guards, model flags,
//!   conversations, shadow scans, and richer transcript signals may fall back
//!   or reject only the evals/options that require them.
//!
//! Generic code resolves an adapter with [`adapter_for`](super::registry::adapter_for)
//! and then calls the trait — so the [`registry`](super::registry) is the one
//! place that names a concrete harness for this surface. The impls live in the
//! per-harness modules ([`claude_code`](super::claude_code),
//! [`codex`](super::codex), [`opencode`](super::opencode)).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::{AvailableSkill, HarnessRunCapabilities, ToolInvocation};
use crate::sandbox::GuardMarker;

use super::skill_shadow::{PluginShadowReport, ShadowSource};
use super::{PermissionDenial, SessionSurface, TranscriptSummary};

/// The role a tool name plays in a harness's vocabulary. A descriptor's roles
/// are validated disjoint (`descriptor::validation::check_tool_roles_disjoint`),
/// so one native name maps to at most one role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRole {
    Write,
    Patch,
    Shell,
    Read,
}

impl ToolRole {
    /// Every role, in `[tools]` key order — the order role lookup and any
    /// message listing several roles walk them, so both read the same way
    /// every run.
    pub const ALL: [Self; 4] = [Self::Write, Self::Patch, Self::Shell, Self::Read];

    /// The role's descriptor spelling — the `[tools]` key, and how grading
    /// evidence names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Patch => "patch",
            Self::Shell => "shell",
            Self::Read => "read",
        }
    }
}

/// One harness's tool-name vocabulary: every name its guard hook payloads or
/// transcript parser can produce, grouped by role. Consumers match against the
/// union across all harnesses ([`all_tool_vocabulary`](super::registry::all_tool_vocabulary)).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolVocabulary {
    /// Tools that write the filesystem with a single target path argument.
    pub write_tools: Vec<String>,
    /// apply_patch-style tools whose payload carries multiple patch targets.
    pub patch_tools: Vec<String>,
    /// Shell-execution tools carrying a `command` argument.
    pub shell_tools: Vec<String>,
    /// Read-only tools carrying a target path argument.
    pub read_tools: Vec<String>,
}

/// A vocabulary declaring nothing: every name is roleless, so a consumer
/// holding it classifies and aliases nothing. Borrowable for `'static`, unlike
/// a `ToolVocabulary::default()` temporary.
pub static EMPTY_TOOL_VOCABULARY: ToolVocabulary = ToolVocabulary {
    write_tools: Vec::new(),
    patch_tools: Vec::new(),
    shell_tools: Vec::new(),
    read_tools: Vec::new(),
};

impl ToolVocabulary {
    /// The role this vocabulary declares for `name`, or `None` when it declares
    /// none — an undeclared name is never given an invented role.
    pub fn role_of(&self, name: &str) -> Option<ToolRole> {
        ToolRole::ALL
            .into_iter()
            .find(|role| self.names_in(*role).iter().any(|tool| tool == name))
    }

    /// Every name this vocabulary declares in `role`, in declaration order.
    pub fn names_in(&self, role: ToolRole) -> &[String] {
        match role {
            ToolRole::Write => &self.write_tools,
            ToolRole::Patch => &self.patch_tools,
            ToolRole::Shell => &self.shell_tools,
            ToolRole::Read => &self.read_tools,
        }
    }
}

/// How per-turn token totals combine for a native resumed conversation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenUsageAggregation {
    /// Each turn reports only its own token usage.
    #[default]
    Sum,
    /// Each turn reports cumulative usage for the native session.
    Last,
}

/// The behavior that varies by harness. Generic dispatch code depends on this
/// trait, never on a concrete harness variant. See the module docs for the
/// baseline-vs-enhancement contract.
pub trait HarnessAdapter {
    // ── Baseline (required) — every harness implements these ────────────────

    /// **Baseline.** The kebab-case identifier used in CLI flags,
    /// `dispatch.json`, and the staged `conditions.json`.
    fn label(&self) -> String;

    /// **Baseline.** The project-local directory staged skills live under for
    /// this harness. Under `--no-stage` nothing is staged into it, so a
    /// baseline harness may point this at any repo-local path its discovery
    /// would read. `None` when the harness declares no skills directory —
    /// native staging is then unavailable and the run preflight forces
    /// `--no-stage` (each SKILL.md is inlined into its dispatch prompt).
    fn skills_dir(&self, repo_root: &Path) -> Option<PathBuf>;

    /// Every project-local skill root this harness may discover. The native
    /// staging root comes first, followed by any cross-harness compatibility
    /// roots declared by the descriptor. This surface is used for codebase
    /// shadow detection and opt-in source exclusion; staging still writes only
    /// to [`skills_dir`](Self::skills_dir).
    fn project_skill_dirs(&self, repo_root: &Path) -> Vec<PathBuf> {
        self.skills_dir(repo_root).into_iter().collect()
    }

    /// Env-relative paths the framework owns in every task environment, as
    /// gitignore-style patterns.
    ///
    /// Staged skills sit *inside* the task repository, so a codebase whose lint
    /// or format step globs the whole tree reports the framework's artifacts as
    /// project failures — and only in the arm that stages a skill. `run` writes
    /// these patterns into the project's own ignore files
    /// ([`crate::workspace::tool_ignore`]) to keep that from happening. The
    /// baseline is [`crate::sandbox::framework_owned_entries`], which every
    /// harness contributes; what a harness adds on top is what it stages.
    fn framework_ignore_paths(&self) -> Vec<String> {
        crate::sandbox::framework_owned_entries().to_vec()
    }

    // ── Run-option capabilities (defaulted) ──────────────────────────────────

    /// The run options the generic `run` preflight may accept for this
    /// harness. The default is the baseline: no write guard, `--bootstrap` and
    /// `--stage-name` allowed alongside `--no-stage`. Override alongside the
    /// enhancement that changes support (e.g. wiring the write guard flips
    /// `supports_guard`).
    fn run_capabilities(&self) -> HarnessRunCapabilities {
        HarnessRunCapabilities {
            supports_guard: false,
            supports_bootstrap_with_no_stage: true,
            supports_stage_name_with_no_stage: true,
        }
    }

    /// The project-local config dir names this harness reads or the adapter
    /// writes (e.g. `.claude`). Staging excludes every harness's config dirs
    /// when copying a skill's sibling assets, so a stray checked-in config dir
    /// never rides into a staged env. The task-repository baseline also force-adds
    /// existing config dirs when a sourced codebase's `.gitignore` covers them.
    /// List the parent of [`skills_dir`](Self::skills_dir) plus any hook/config
    /// dirs the adapter writes.
    fn config_dir_names(&self) -> Vec<String> {
        Vec::new()
    }

    /// Environment defaults applied only to eval-agent dispatches. Generic run
    /// orchestration merges `run --agent-env` values over this map.
    fn dispatch_environment(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    /// The tool names this harness's guard hook payloads and parsed transcripts
    /// use, grouped by role. Via
    /// [`all_tool_vocabulary`](super::registry::all_tool_vocabulary) this feeds the guard
    /// arbiter's tool classification and detect-stray-writes' invocation audit,
    /// so list every name this harness's surfaces produce — even names another
    /// harness also uses; the union dedups. Default empty: a harness with no
    /// guard and no transcript parser contributes nothing.
    fn tool_vocabulary(&self) -> ToolVocabulary {
        ToolVocabulary::default()
    }

    // ── Enhancement: native skill staging (defaulted) ────────────────────────
    // Fallback without it: `--no-stage` inlines each SKILL.md into its
    // dispatch prompt instead of staging files for native discovery.

    /// **Enhancement: native staging.** Build the conspicuous staged-skill
    /// slug. The default underscore form is fine for any harness without
    /// naming rules; a harness with constrained skill names (e.g. OpenCode)
    /// overrides it. `prefix` must be preserved so cleanup prefix-scans still
    /// find the staged dir.
    fn staged_slug(
        &self,
        prefix: &str,
        iteration: u32,
        condition: &str,
        skill_name: &str,
    ) -> String {
        format!("{prefix}{iteration}-{condition}__{skill_name}")
    }

    /// **Enhancement: native staging.** Validate a staged-skill identifier
    /// (generated slug or `--stage-name` override) against this harness's
    /// naming rules. The default accepts anything.
    fn validate_stage_name(&self, _name: &str) -> Result<(), String> {
        Ok(())
    }

    /// **Enhancement: native staging.** Whether a staged skill's frontmatter
    /// `name:` is rewritten to its slug so the harness's repo-local discovery
    /// resolves the staged copy.
    fn rewrites_frontmatter_name(&self) -> bool {
        false
    }

    /// **Enhancement: native staging.** Whether the skill-under-test is
    /// advertised in the available-skills block under its staged slug (vs. its
    /// natural name). True for Codex and OpenCode, whose repo-local discovery
    /// keys on the rewritten frontmatter name.
    fn advertises_staged_slug_name(&self) -> bool {
        false
    }

    /// **Enhancement: native staging.** Render the discoverable skills the way
    /// this harness natively surfaces them (e.g. Claude Code's Skill-tool
    /// list, Codex's `## Skills`, OpenCode's `<available_skills>` XML). The
    /// default is a neutral bulleted list.
    fn render_available_skills_block(&self, skills: &[AvailableSkill]) -> String {
        super::skills_block::render_skills_block(
            super::skills_block::DEFAULT_HEADER,
            super::skills_block::DEFAULT_ITEM,
            "",
            skills,
        )
    }

    /// **Enhancement: native staging.** How a staged skill is described as
    /// discoverable in the neutral slug-disambiguation line (e.g. "via the
    /// Skill tool").
    fn skill_surface_phrase(&self) -> String {
        "as a discoverable skill".to_string()
    }

    /// **Enhancement: native staging.** The lead-in for the fallback "read the
    /// skill from `<path>`" instruction when the staged identifier can't be
    /// resolved.
    fn skill_unresolved_phrase(&self) -> String {
        "If the staged skill cannot be resolved".to_string()
    }

    // ── Runner requirement: transcript ingest (defaulted) ───────────────────
    // Run preflight rejects a harness without this capability. The default
    // remains unwired so a partial descriptor can still be linted and shown.

    /// **Runner requirement: transcript ingest.** The filename (under a task's
    /// `outputs/turn-N/` dir) this harness's CLI writes the captured transcript
    /// to. `None` means the descriptor is not runner-ready.
    fn cli_events_filename(&self) -> Option<String> {
        None
    }

    /// **Enhancement: transcript ingest.** Parse the events file this
    /// harness's one-shot CLI wrote (the captured transcript) into ordered
    /// tool invocations.
    fn parse_cli_events(&self, _path: &Path) -> io::Result<Vec<ToolInvocation>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "transcript ingest is not wired for the {} harness",
                self.label()
            ),
        ))
    }

    /// **Enhancement: transcript ingest.** The full-summary counterpart of
    /// [`parse_cli_events`](Self::parse_cli_events): tool invocations, deduped
    /// token usage, duration, and final message text.
    fn parse_cli_events_full(&self, _path: &Path) -> io::Result<TranscriptSummary> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "transcript ingest is not wired for the {} harness",
                self.label()
            ),
        ))
    }

    /// **Enhancement: transcript ingest.** The deterministic skill-invocation
    /// signature the `__skill_invoked` meta-check matches: `(tool name, arg
    /// carrying the staged slug)` — Claude Code's `Skill`/`skill`, OpenCode's
    /// `skill`/`name`. `None` for Codex (its JSONL has no skill-tool event),
    /// which routes the meta-check to the LLM-judge fallback.
    fn transcript_skill_invocation(&self) -> Option<(String, String)> {
        Some(("Skill".to_string(), "skill".to_string()))
    }

    /// **Enhancement: transcript denial reader.** Whether this harness's
    /// transcript identifies tool calls it refused to run. `false` for
    /// harnesses whose refusals are not distinguishable from ordinary tool
    /// errors — `ingest` then writes no `permission-denials.json` and
    /// `aggregate` raises no permission-denial validity warning, so a silently
    /// degraded run is only visible in the transcripts.
    fn surfaces_permission_denials(&self) -> bool {
        false
    }

    /// **Enhancement: transcript denial reader.** The refused tool calls in a
    /// captured events file. Unlike [`parse_cli_events`](Self::parse_cli_events)
    /// the default is an empty vec, not an `Unsupported` error: no detection is
    /// a supported fallback, and the pipeline treats "none reported" and
    /// "cannot report" alike rather than failing ingest.
    fn parse_permission_denials(&self, _path: &Path) -> io::Result<Vec<PermissionDenial>> {
        Ok(Vec::new())
    }

    /// **Enhancement: session surface.** Whether this harness's transcript
    /// reports the skills and plugins the session could actually discover.
    /// `false` for harnesses whose captures carry no such roster — `ingest` then
    /// writes no `session-surface.json` and shadow findings stay unverified,
    /// leaving `[shadow] isolates_live_sources` as the operator's only way to
    /// record that a dispatch was isolated.
    fn surfaces_session_surface(&self) -> bool {
        false
    }

    /// **Enhancement: session surface.** The skill/plugin surface one captured
    /// events file reports. `Ok(None)` means the capture says nothing knowable;
    /// only a `Some` with an empty roster can refute a live-source finding, so
    /// the default is `None` rather than an empty surface.
    fn parse_session_surface(&self, _path: &Path) -> io::Result<Option<SessionSurface>> {
        Ok(None)
    }

    // ── Enhancement: model flag (defaulted) ──────────────────────────────────
    // Fallback without it: `--agent-model` / `--judge-model` are recorded as
    // provenance only; dispatches run on the harness's default model.

    /// **Enhancement: model flag.** The native model-selection flag accepted
    /// by this harness's CLI. `None` means no model-selection support is
    /// wired.
    fn cli_model_flag(&self) -> Option<String> {
        None
    }

    // ── Enhancement: write guard (defaulted) ─────────────────────────────────
    // Fallback without it: the `detect-stray-writes` post-pass (folded into
    // `ingest`) audits out-of-bounds writes after the fact.

    /// **Enhancement: write guard.** Arm the write guard using this harness's
    /// native pre-tool hook surface, returning the staged marker path. The
    /// guard's allowed roots are derived from `stage_root` (the isolated env /
    /// agent cwd), so it bounds the agent to the same env boundary that
    /// isolates its reads.
    fn install_guard(
        &self,
        _stage_root: &Path,
        _guard_exe: &Path,
        _ttl: Option<Duration>,
        _guard_policy: &crate::core::GuardPolicyConfig,
    ) -> io::Result<PathBuf> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("--guard is not supported for the {} harness", self.label()),
        ))
    }

    /// **Enhancement: write guard.** The banner printed after `--guard`
    /// successfully arms, describing the harness's native hook surface and how
    /// to remove it. `None` for a harness with no write guard (its
    /// [`install_guard`](Self::install_guard) errors), in which case no banner
    /// is printed.
    fn guard_armed_message(&self) -> Option<String> {
        None
    }

    /// **Enhancement: write guard.** Evaluate a PreToolUse hook `payload`
    /// against `marker`, returning the serialized deny verdict to print on
    /// stdout, or `None` to allow. The default fails open — a harness with no
    /// guard never denies — matching the hook entry points' contract that a
    /// guard invocation can never brick a session.
    fn guard_verdict(&self, _payload: &str, _marker: Option<GuardMarker>) -> Option<String> {
        None
    }

    /// **Enhancement: write guard.** A hook-config dir the guard install
    /// created outside [`skills_dir`](Self::skills_dir) (e.g. Codex's
    /// `.codex/`), which teardown prunes when restoring the original config
    /// leaves it empty. `None` when the guard writes only under existing dirs.
    fn guard_hook_cleanup_dir(&self, _stage_root: &Path) -> Option<PathBuf> {
        None
    }

    // ── Enhancement: shadow preflight (defaulted) ────────────────────────────
    // Fallback without it: no preflight — the run proceeds with no shadow
    // report, exactly as for a harness whose dispatches load nothing global.

    /// **Enhancement: shadow preflight.** Detect staged skill names that are
    /// also discoverable from the operator's live environment (e.g. Claude
    /// Code's enabled plugins or global skills dir), which could contaminate
    /// the with/without comparison unless dispatches isolate the source.
    /// `scan_root` is a real staged env root — its project-local settings
    /// participate in detection. `None` when the harness has no shadow
    /// preflight (the default) or nothing is shadowed.
    fn detect_shadowed_skills(
        &self,
        _scan_root: &Path,
        _staged_skill_names: &[&str],
    ) -> Option<PluginShadowReport> {
        None
    }

    /// **Enhancement: shadow preflight.** Whether the resolved descriptor
    /// asserts that every live source the preflight can report is excluded
    /// from every eval-agent dispatch. The default preserves warning behavior.
    fn isolates_live_sources(&self) -> bool {
        false
    }

    /// **Enhancement: shadow preflight.** Resolve duplicate runtime ids for one
    /// concrete comparison cell. The generic fallback records coexistence.
    fn resolve_shadow_sources(&self, _scan_root: &Path, sources: &mut [ShadowSource]) {
        super::skill_shadow::resolve_as_coexisting(sources);
    }

    /// **Enhancement: shadow preflight.** Format the shared runner banner for
    /// a report. Whether the banner promises a verified verdict follows this
    /// adapter's own
    /// [`surfaces_session_surface`](Self::surfaces_session_surface), so a caller
    /// cannot accidentally claim verification a harness can't deliver.
    fn format_shadow_banner(&self, report: &PluginShadowReport) -> String {
        super::skill_shadow::format_shadow_banner_with_verification(
            report,
            self.surfaces_session_surface(),
        )
    }

    /// **Enhancement: shadow preflight.** Format shared aggregate validity
    /// warnings for a report.
    fn shadow_validity_warnings(&self, report: &PluginShadowReport) -> Vec<String> {
        super::skill_shadow::shadow_validity_warnings(report)
    }

    // ── Enhancement: plan-mode context (defaulted) ───────────────────────────

    /// **Enhancement: plan-mode context.** Wrap a plan-mode profile as an
    /// operating-context layer. The shared `<system-reminder>` default
    /// usually suffices; a harness with a real native plan mode could inject
    /// it differently.
    fn render_plan_mode_context(&self, profile_text: &str) -> String {
        let trimmed = profile_text.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        format!("<system-reminder>\n{trimmed}\n</system-reminder>")
    }

    // ── Runner requirement: dispatch commands (defaulted) ────────────────────
    // There is no fallback: without an exec template the runner has nothing to
    // spawn, so `run` rejects that harness during preflight.

    /// **Runner requirement: dispatch commands.** Whether a per-task exec command is
    /// wired (the descriptor's `[dispatch] exec_template`). `false` means
    /// `eval-magic dispatch` has nothing to run for this harness, and the `run`
    /// preflight rejects it.
    fn has_dispatch_recipes(&self) -> bool {
        false
    }

    /// Render the harness's one-shot CLI command for a task. Angle-bracket
    /// task paths remain for the caller to substitute.
    fn cli_exec_command(
        &self,
        _guard: bool,
        _agent_model: Option<&str>,
        _agent_env: &BTreeMap<String, String>,
    ) -> Option<String> {
        None
    }

    /// **Enhancement: native conversation resume.** Whether the harness can
    /// continue a captured native session for scripted follow-up turns.
    fn has_conversation_resume(&self) -> bool {
        false
    }

    /// How transcript token totals combine across resumed conversation turns.
    fn conversation_token_usage_aggregation(&self) -> TokenUsageAggregation {
        TokenUsageAggregation::Sum
    }

    /// Render one same-session follow-up command. In addition to the usual
    /// angle-bracket task paths, `{session_arg}` and `{prompt_arg}` remain for
    /// the conversation driver to fill with shell-quoted values.
    fn cli_resume_command(
        &self,
        _guard: bool,
        _agent_model: Option<&str>,
        _agent_env: &BTreeMap<String, String>,
    ) -> Option<String> {
        None
    }

    /// **Enhancement: dispatch commands.** The `Next:` guidance printed after
    /// `run`: the dispatch and ingest commands for this harness. Empty when the
    /// descriptor wires no `next_steps_template`.
    fn cli_next_steps(&self, _ctx: CliDispatchContext<'_>) -> String {
        String::new()
    }

    /// **Enhancement: dispatch commands.** Extra `dispatch-manifest.md` lines
    /// describing what the runner will spawn for this harness (the command
    /// template and any ingest note). `None` when the harness contributes no
    /// manifest section.
    fn cli_manifest_section(&self, _ctx: CliManifestContext<'_>) -> Option<Vec<String>> {
        None
    }
}

/// The shared (human-followed) `RUNBOOK.md` template used by every run,
/// regardless of harness (Claude Code, Codex, OpenCode).
pub const RUNBOOK_TEMPLATE: &str = include_str!("../../profiles/shared/runbook.md");

/// Context for rendering a harness's one-shot CLI agent-dispatch guidance.
#[derive(Debug, Clone, Copy)]
pub struct CliDispatchContext<'a> {
    pub guard: bool,
    pub target_args: &'a str,
    pub iteration: u32,
    pub agent_model: Option<&'a str>,
    pub agent_env: &'a BTreeMap<String, String>,
}

/// Context for rendering a harness's `dispatch-manifest.md` CLI recipe.
#[derive(Debug, Clone, Copy)]
pub struct CliManifestContext<'a> {
    pub guard: bool,
    pub agent_model: Option<&'a str>,
    pub agent_env: &'a BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::registry::adapter_for;
    use crate::core::Harness;

    // Cross-method invariants (guard/banner lockstep, hook-matcher ⊆
    // vocabulary, slug ↔ naming rules, …) are enforced at descriptor load
    // time in `descriptor::validation`; only per-value pins live here.

    #[test]
    fn detect_shadowed_skills_defaults_to_none_for_harnesses_without_a_preflight() {
        // Every built-in harness declares a shadow preflight today; a
        // descriptor without [shadow] (the baseline BYOH shape) gets none.
        let descriptor = crate::adapters::descriptor::load_descriptor(
            "label = \"demo\"\nskills_dir = \".demo/skills\"\nconfig_dirs = [\".demo\"]\n",
            "test.toml",
        )
        .unwrap();
        let adapter =
            crate::adapters::descriptor_adapter::DescriptorAdapter::from_descriptor(descriptor);
        assert_eq!(
            adapter.detect_shadowed_skills(Path::new("/nonexistent"), &["any-skill"]),
            None
        );
    }

    #[test]
    fn has_dispatch_recipes_matches_the_readme_support_table() {
        assert!(adapter_for(Harness::resolve("claude-code").unwrap()).has_dispatch_recipes());
        assert!(adapter_for(Harness::resolve("codex").unwrap()).has_dispatch_recipes());
        assert!(adapter_for(Harness::resolve("opencode").unwrap()).has_dispatch_recipes());
    }

    #[test]
    fn skills_dir_is_harness_native() {
        let root = Path::new("/repo");
        assert_eq!(
            adapter_for(Harness::resolve("claude-code").unwrap()).skills_dir(root),
            Some(root.join(".claude").join("skills"))
        );
        assert_eq!(
            adapter_for(Harness::resolve("codex").unwrap()).skills_dir(root),
            Some(root.join(".agents").join("skills"))
        );
        assert_eq!(
            adapter_for(Harness::resolve("opencode").unwrap()).skills_dir(root),
            Some(root.join(".opencode").join("skills"))
        );
    }

    #[test]
    fn project_skill_dirs_include_cross_harness_roots_declared_by_the_descriptor() {
        let root = Path::new("/repo");
        assert_eq!(
            adapter_for(Harness::resolve("claude-code").unwrap()).project_skill_dirs(root),
            vec![root.join(".claude/skills")]
        );
        assert_eq!(
            adapter_for(Harness::resolve("opencode").unwrap()).project_skill_dirs(root),
            vec![
                root.join(".opencode/skills"),
                root.join(".claude/skills"),
                root.join(".agents/skills"),
            ]
        );
    }

    #[test]
    fn framework_ignore_paths_cover_the_staged_skills_the_guard_file_and_what_the_framework_owns() {
        assert_eq!(
            adapter_for(Harness::resolve("claude-code").unwrap()).framework_ignore_paths(),
            vec![
                "/.eval-magic-outputs/".to_string(),
                "/tmp/".to_string(),
                "/.claude/skills/".to_string(),
                "/.claude/settings.local.json".to_string(),
            ]
        );
        // A plugin-engine harness contributes its plugin file, not a hooks file.
        assert_eq!(
            adapter_for(Harness::resolve("opencode").unwrap()).framework_ignore_paths(),
            vec![
                "/.eval-magic-outputs/".to_string(),
                "/tmp/".to_string(),
                "/.opencode/skills/".to_string(),
                "/.opencode/plugins/slow-powers-eval-guard.js".to_string(),
            ]
        );
        assert_eq!(
            adapter_for(Harness::resolve("codex").unwrap()).framework_ignore_paths(),
            vec![
                "/.eval-magic-outputs/".to_string(),
                "/tmp/".to_string(),
                "/.agents/skills/".to_string(),
                "/.codex/hooks.json".to_string(),
            ]
        );
    }

    #[test]
    fn framework_ignore_paths_omit_what_a_bare_descriptor_never_stages() {
        let descriptor =
            crate::adapters::descriptor::load_descriptor("label = \"bare\"\n", "test.toml")
                .unwrap();
        let adapter =
            crate::adapters::descriptor_adapter::DescriptorAdapter::from_descriptor(descriptor);

        assert_eq!(
            adapter.framework_ignore_paths(),
            vec!["/.eval-magic-outputs/".to_string(), "/tmp/".to_string()]
        );
    }

    #[test]
    fn only_codex_and_opencode_rewrite_frontmatter() {
        assert!(!adapter_for(Harness::resolve("claude-code").unwrap()).rewrites_frontmatter_name());
        assert!(adapter_for(Harness::resolve("codex").unwrap()).rewrites_frontmatter_name());
        assert!(adapter_for(Harness::resolve("opencode").unwrap()).rewrites_frontmatter_name());
    }

    #[test]
    fn skill_invocation_signatures_are_harness_native() {
        // (tool name, slug-carrying arg) the `__skill_invoked` meta-check
        // matches; None routes the check to the LLM-judge fallback.
        assert_eq!(
            adapter_for(Harness::resolve("claude-code").unwrap()).transcript_skill_invocation(),
            Some(("Skill".to_string(), "skill".to_string()))
        );
        assert_eq!(
            adapter_for(Harness::resolve("codex").unwrap()).transcript_skill_invocation(),
            None
        );
        assert_eq!(
            adapter_for(Harness::resolve("opencode").unwrap()).transcript_skill_invocation(),
            Some(("skill".to_string(), "name".to_string()))
        );
    }

    #[test]
    fn claude_codex_and_opencode_surface_permission_denials() {
        // Each harness encodes a refused tool call differently, so detection is
        // opt-in per named reader. The three parser-backed built-ins select one
        // today; Cline's declarative extract tier cannot distinguish a refusal
        // from an ordinary tool error (a `cline-json` parser is the tracked
        // gap), and any future harness in that position leaves the default
        // `false` and reports nothing rather than guessing — `aggregate` then
        // raises no permission-denial warning.
        assert!(
            adapter_for(Harness::resolve("claude-code").unwrap()).surfaces_permission_denials()
        );
        assert!(adapter_for(Harness::resolve("codex").unwrap()).surfaces_permission_denials());
        assert!(adapter_for(Harness::resolve("opencode").unwrap()).surfaces_permission_denials());
    }

    #[test]
    fn a_deny_less_transcript_parses_no_denials_for_every_detecting_harness() {
        // "None reported" is a supported, distinguishable outcome — not an
        // error. Each detecting reader, given a transcript whose tool calls all
        // completed or failed for ordinary (non-permission) reasons, yields an
        // empty vec; the per-reader suites cover the false-positive guard.
        // Harnesses without a denial reader (Cline's declarative extract tier
        // cannot distinguish refusals) are skipped rather than expected to
        // detect.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(&path, "{\"type\":\"turn.completed\"}\n").unwrap();
        for harness in Harness::known() {
            let adapter = adapter_for(harness);
            if !adapter.surfaces_permission_denials() {
                continue;
            }
            assert_eq!(
                adapter.parse_permission_denials(&path).unwrap(),
                Vec::new(),
                "{harness:?} reported a denial from a denial-less transcript"
            );
        }
    }

    #[test]
    fn only_claude_code_reports_a_session_surface_today() {
        // Claude Code's descriptor maps its `init` roster declaratively. Codex
        // reports only `thread_id` on `thread.started`, OpenCode's envelope
        // carries no roster at all, and Cline's `agent_event` stream has no
        // roster record either, so none of them can supply this evidence and
        // their shadow findings stay unverified rather than being wrongly
        // refuted.
        assert!(
            adapter_for(Harness::resolve("claude-code").unwrap()).surfaces_session_surface(),
            "claude-code's init event carries the surface"
        );
        for harness in ["cline", "codex", "opencode"] {
            assert!(
                !adapter_for(Harness::resolve(harness).unwrap()).surfaces_session_surface(),
                "{harness} transcripts carry no skill/plugin roster"
            );
        }
    }

    #[test]
    fn a_surface_less_transcript_parses_to_none_for_every_harness() {
        // No roster in the capture must read as "no evidence" (`None`), never as
        // an empty surface — an empty surface would refute a live-source finding.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(&path, "{\"type\":\"turn.completed\"}\n").unwrap();
        for harness in Harness::known() {
            assert_eq!(
                adapter_for(harness).parse_session_surface(&path).unwrap(),
                None,
                "{harness:?} invented a surface from a roster-less transcript"
            );
        }
    }

    #[test]
    fn plan_mode_context_wraps_in_system_reminder_for_every_harness() {
        for h in Harness::known() {
            let out = adapter_for(h).render_plan_mode_context("BODY");
            assert_eq!(out, "<system-reminder>\nBODY\n</system-reminder>");
            assert_eq!(adapter_for(h).render_plan_mode_context("   "), "");
            assert_eq!(
                adapter_for(h).render_plan_mode_context("\n\n  BODY  \n\n"),
                "<system-reminder>\nBODY\n</system-reminder>"
            );
        }
    }

    #[test]
    fn run_capabilities_capture_run_option_support_by_harness() {
        let claude = adapter_for(Harness::resolve("claude-code").unwrap()).run_capabilities();
        assert!(claude.supports_guard);
        assert!(claude.supports_bootstrap_with_no_stage);
        assert!(claude.supports_stage_name_with_no_stage);

        let codex = adapter_for(Harness::resolve("codex").unwrap()).run_capabilities();
        assert!(codex.supports_guard);
        assert!(codex.supports_bootstrap_with_no_stage);
        assert!(!codex.supports_stage_name_with_no_stage);

        let opencode = adapter_for(Harness::resolve("opencode").unwrap()).run_capabilities();
        assert!(opencode.supports_guard);
        assert!(opencode.supports_bootstrap_with_no_stage);
        assert!(opencode.supports_stage_name_with_no_stage);
    }

    #[test]
    fn guard_armed_message_is_harness_specific() {
        // The post-arm `--guard` banner names the harness's native hook surface,
        // so it lives behind the adapter rather than in generic run code.
        let claude = adapter_for(Harness::resolve("claude-code").unwrap())
            .guard_armed_message()
            .expect("claude code has a write guard");
        assert!(
            claude.contains(".claude/settings.local.json"),
            "claude banner names its hook file: {claude}"
        );

        let codex = adapter_for(Harness::resolve("codex").unwrap())
            .guard_armed_message()
            .expect("codex has a write guard");
        assert!(
            codex.contains(".codex/hooks.json"),
            "codex banner names its hook file: {codex}"
        );

        let opencode = adapter_for(Harness::resolve("opencode").unwrap())
            .guard_armed_message()
            .expect("opencode has a write guard");
        assert!(
            opencode.contains(".opencode/plugins/slow-powers-eval-guard.js"),
            "opencode banner names its plugin file: {opencode}"
        );
    }

    #[test]
    fn staged_slug_default_and_opencode_override_preserve_the_prefix() {
        let prefix = "slow-powers-eval-";
        assert_eq!(
            adapter_for(Harness::resolve("claude-code").unwrap()).staged_slug(
                prefix,
                2,
                "with_skill",
                "my-skill"
            ),
            "slow-powers-eval-2-with_skill__my-skill"
        );
        assert_eq!(
            adapter_for(Harness::resolve("opencode").unwrap()).staged_slug(
                prefix,
                2,
                "with_skill",
                "my-skill"
            ),
            "slow-powers-eval-2-with-skill-my-skill"
        );
    }

    fn vocabulary() -> ToolVocabulary {
        ToolVocabulary {
            write_tools: vec!["Edit".into(), "Write".into(), "file_change".into()],
            patch_tools: vec!["apply_patch".into()],
            shell_tools: vec!["Bash".into(), "command_execution".into()],
            read_tools: vec![],
        }
    }

    #[test]
    fn role_of_finds_the_role_declaring_each_name() {
        let vocabulary = vocabulary();
        assert_eq!(vocabulary.role_of("file_change"), Some(ToolRole::Write));
        assert_eq!(vocabulary.role_of("apply_patch"), Some(ToolRole::Patch));
        assert_eq!(
            vocabulary.role_of("command_execution"),
            Some(ToolRole::Shell)
        );
    }

    #[test]
    fn role_of_is_none_for_a_name_the_vocabulary_does_not_declare() {
        // Codex declares no read tools, so a read-role name is unknown to it —
        // and an undeclared name must not be given an invented role.
        assert_eq!(vocabulary().role_of("Read"), None);
        assert_eq!(vocabulary().role_of("WebFetch"), None);
    }

    #[test]
    fn names_in_lists_every_name_declared_for_the_role() {
        let vocabulary = vocabulary();
        assert_eq!(
            vocabulary.names_in(ToolRole::Shell),
            ["Bash", "command_execution"]
        );
        assert!(vocabulary.names_in(ToolRole::Read).is_empty());
    }

    #[test]
    fn tool_role_renders_its_descriptor_spelling() {
        assert_eq!(ToolRole::Write.as_str(), "write");
        assert_eq!(ToolRole::Patch.as_str(), "patch");
        assert_eq!(ToolRole::Shell.as_str(), "shell");
        assert_eq!(ToolRole::Read.as_str(), "read");
    }
}
