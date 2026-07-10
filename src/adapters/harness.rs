//! The harness adapter API — the single seam between generic dispatch code and
//! harness-specific behavior.
//!
//! The trait is tiered into a **baseline** every harness must implement and
//! **enhancements** that raise fidelity when a harness has the native support:
//!
//! - **Baseline (required):** [`label`](HarnessAdapter::label) and
//!   [`skills_dir`](HarnessAdapter::skills_dir). A new harness compiles with
//!   just these two methods; dispatched through its one-shot CLI (with
//!   `--no-stage` inlining the skill when native staging isn't wired), it
//!   already supports `llm_judge` grading and the `detect-stray-writes`
//!   post-pass.
//! - **Enhancements (defaulted):** every other method has a default — either a
//!   working generic fallback (e.g. the plain available-skills block) or an
//!   `Unsupported` error naming the enhancement it belongs to (e.g. transcript
//!   ingest, the write guard). Override the methods of an enhancement to wire
//!   it for a harness.
//!
//! Generic code resolves an adapter with [`adapter_for`] and then calls the
//! trait — so [`adapter_for`] is the one place that names a concrete harness
//! for this surface. The impls live in the per-harness modules
//! ([`claude_code`](super::claude_code), [`codex`](super::codex),
//! [`opencode`](super::opencode)).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use crate::core::{AvailableSkill, Harness, HarnessRunCapabilities, ToolInvocation};

use super::TranscriptSummary;
use super::descriptor::{EMBEDDED_DESCRIPTORS, load_descriptor};
use super::descriptor_adapter::DescriptorAdapter;
use super::skill_shadow::PluginShadowReport;

/// One harness's tool-name vocabulary: every name its guard hook payloads or
/// transcript parser can produce, grouped by role. Consumers match against the
/// union across all harnesses ([`all_tool_vocabulary`]).
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
    /// would read.
    fn skills_dir(&self, repo_root: &Path) -> PathBuf;

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
    /// never rides into a staged env. Via [`all_config_dir_names`] this list
    /// also feeds the guard's Bash tamper rule and detect-stray-writes'
    /// staging-dir lookbehind, so adding a dir here automatically grows the
    /// write-guard's deny surface. List the parent of
    /// [`skills_dir`](Self::skills_dir) plus any hook/config dirs the adapter
    /// writes.
    fn config_dir_names(&self) -> Vec<String> {
        Vec::new()
    }

    /// The tool names this harness's guard hook payloads and parsed transcripts
    /// use, grouped by role. Via [`all_tool_vocabulary`] this feeds the guard
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
    /// natural name). True for Codex, whose repo-local discovery keys on the
    /// rewritten frontmatter name. (OpenCode also rewrites the frontmatter to
    /// the slug yet still advertises the natural name — a known inconsistency
    /// tracked for a separate fix.)
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

    // ── Enhancement: transcript parser (defaulted) ───────────────────────────
    // Fallback without it: `transcript_check` assertions grade as
    // unverifiable, `llm_judge` carries the grading, token/cost/duration go
    // unrecorded, and run records are assembled by hand (or from
    // `outputs/final-message.md`) instead of auto-ingested.

    /// **Enhancement: transcript parser.** The filename (under a task's
    /// `outputs/` dir) this harness's one-shot CLI writes the captured
    /// transcript to. `None` when no transcript ingest is wired — the ingest
    /// pipeline then never calls the parsers below.
    fn cli_events_filename(&self) -> Option<String> {
        None
    }

    /// **Enhancement: transcript parser.** Parse the events file this
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

    /// **Enhancement: transcript parser.** The full-summary counterpart of
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

    /// **Enhancement: transcript parser.** Whether the parsed transcript
    /// exposes a deterministic skill-invocation event the `__skill_invoked`
    /// meta-check can match. False for Codex (its JSONL has no skill-tool
    /// event), which routes the meta-check to the LLM-judge fallback.
    fn transcript_surfaces_skill_invocation(&self) -> bool {
        true
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
    /// Code's enabled plugins or global skills dir), which contaminates the
    /// with/without comparison. `scan_root` is a real staged env root — its
    /// project-local settings participate in detection. `None` when the
    /// harness has no shadow preflight (the default) or nothing is shadowed.
    fn detect_shadowed_skills(
        &self,
        _scan_root: &Path,
        _staged_skill_names: &[&str],
    ) -> Option<PluginShadowReport> {
        None
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

    // ── Enhancement: dispatch recipes (defaulted) ────────────────────────────
    // Fallback without them: `run` prints the generic handoff and the runbook
    // carries no copy-pasteable per-task command.

    /// **Enhancement: dispatch recipes.** The `Next:` guidance printed after
    /// `run`: how to dispatch each task through this harness's one-shot CLI
    /// and then ingest. Empty when no dispatch recipe is wired.
    fn cli_next_steps(&self, _ctx: CliDispatchContext<'_>) -> String {
        String::new()
    }

    /// **Enhancement: dispatch recipes.** Extra `dispatch-manifest.md` lines
    /// describing this harness's dispatch recipe (command template, parallel
    /// recipe, ingest note). `None` when the harness contributes no manifest
    /// section.
    fn cli_manifest_section(&self, _ctx: CliManifestContext<'_>) -> Option<Vec<String>> {
        None
    }

    /// **Enhancement: dispatch recipes.** The post-`grade` / post-`ingest`
    /// judge dispatch guidance for this harness. `None` leaves the generic
    /// judge handoff in place.
    fn cli_judge_next_steps(&self, _ctx: CliJudgeContext<'_>) -> Option<String> {
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
}

/// Context for rendering a harness's `dispatch-manifest.md` CLI recipe.
#[derive(Debug, Clone, Copy)]
pub struct CliManifestContext<'a> {
    pub guard: bool,
    pub agent_model: Option<&'a str>,
}

/// Context for rendering a harness's one-shot CLI judge-dispatch guidance.
#[derive(Debug, Clone, Copy)]
pub struct CliJudgeContext<'a> {
    pub guard: bool,
    pub iteration_dir: &'a Path,
}

/// The built-in adapters, one descriptor-backed adapter per [`Harness`]
/// variant, loaded once from the embedded descriptor files.
struct Registry {
    claude_code: DescriptorAdapter,
    codex: DescriptorAdapter,
    opencode: DescriptorAdapter,
}

/// Built on first `adapter_for` call. The embedded descriptors are bundled and
/// known-valid, so a load failure here is a programmer error (a bad descriptor
/// edit) and panics — mirroring the bundled-schema panics in
/// `validation::schema` — with the descriptor's own actionable message.
static REGISTRY: LazyLock<Registry> = LazyLock::new(|| Registry {
    claude_code: build_embedded(Harness::ClaudeCode, EMBEDDED_DESCRIPTORS[0]),
    codex: build_embedded(Harness::Codex, EMBEDDED_DESCRIPTORS[1]),
    opencode: build_embedded(Harness::OpenCode, EMBEDDED_DESCRIPTORS[2]),
});

fn build_embedded(harness: Harness, (source, toml_src): (&str, &str)) -> DescriptorAdapter {
    let descriptor = load_descriptor(toml_src, source)
        .unwrap_or_else(|e| panic!("bundled harness descriptor is invalid: {e}"));
    // Label ↔ variant lockstep: the descriptor's label must be the variant's
    // serde/CLI identifier, or `--harness <label>` and the descriptor would
    // name different harnesses.
    let expected = serde_json::to_value(harness)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .expect("Harness serializes to its kebab-case identifier");
    assert_eq!(
        descriptor.label, expected,
        "bundled descriptor {source} label {:?} does not match the {expected} harness identifier",
        descriptor.label
    );
    DescriptorAdapter::from_descriptor(descriptor)
}

/// Resolve the adapter for a [`Harness`]. This is the single dispatch point on
/// the harness variant for all harness-specific behavior; every other module
/// goes through the returned trait object.
pub fn adapter_for(harness: Harness) -> &'static dyn HarnessAdapter {
    match harness {
        Harness::ClaudeCode => &REGISTRY.claude_code,
        Harness::Codex => &REGISTRY.codex,
        Harness::OpenCode => &REGISTRY.opencode,
    }
}

/// The union of every harness's project-local config dir names (sorted,
/// deduplicated): the dirs harness-agnostic code must treat as protected —
/// staging's sibling-asset filter, the guard's Bash tamper rule, and
/// detect-stray-writes' staging-dir lookbehind.
pub fn all_config_dir_names() -> Vec<String> {
    let mut names: Vec<String> = Harness::ALL
        .iter()
        .flat_map(|&h| adapter_for(h).config_dir_names())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// The union of every harness's tool vocabulary (each list sorted,
/// deduplicated). Computed once behind a `LazyLock` — the guard arbiter
/// consults it on every hooked tool call.
pub fn all_tool_vocabulary() -> &'static ToolVocabulary {
    static ALL: LazyLock<ToolVocabulary> = LazyLock::new(|| {
        let mut union = ToolVocabulary::default();
        for &h in Harness::ALL.iter() {
            let vocab = adapter_for(h).tool_vocabulary();
            union.write_tools.extend(vocab.write_tools);
            union.patch_tools.extend(vocab.patch_tools);
            union.shell_tools.extend(vocab.shell_tools);
            union.read_tools.extend(vocab.read_tools);
        }
        for list in [
            &mut union.write_tools,
            &mut union.patch_tools,
            &mut union.shell_tools,
            &mut union.read_tools,
        ] {
            list.sort_unstable();
            list.dedup();
        }
        union
    });
    &ALL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_config_dir_names_unions_every_adapter() {
        assert_eq!(
            all_config_dir_names(),
            [".agents", ".claude", ".codex", ".opencode"]
        );
    }

    #[test]
    fn all_tool_vocabulary_unions_every_adapter() {
        let vocab = all_tool_vocabulary();
        assert_eq!(
            vocab.write_tools,
            ["Edit", "MultiEdit", "NotebookEdit", "Write", "file_change"]
        );
        assert_eq!(vocab.patch_tools, ["apply_patch"]);
        assert_eq!(vocab.shell_tools, ["Bash", "command_execution"]);
        assert_eq!(vocab.read_tools, ["Glob", "Grep", "Read"]);
    }

    // The old cross-method invariant tests (guard/banner lockstep, hook-matcher
    // ⊆ vocabulary, transcript ⇒ stray-writes vocabulary, role disjointness,
    // slug ↔ naming rules, config-dirs ⊇ skills-dir parent) now run at
    // descriptor load time — see `descriptor::validate_descriptor` and its
    // per-invariant rejection tests. The registry's LazyLock panics on any
    // violation, and `descriptor::tests::embedded_descriptors_load_and_validate`
    // keeps that on the CI path. What stays here are the per-value pins.

    #[test]
    fn detect_shadowed_skills_defaults_to_none_for_harnesses_without_a_preflight() {
        for h in [Harness::Codex, Harness::OpenCode] {
            assert_eq!(
                adapter_for(h).detect_shadowed_skills(Path::new("/nonexistent"), &["any-skill"]),
                None
            );
        }
    }

    #[test]
    fn labels_match_kebab_case_identifiers() {
        assert_eq!(adapter_for(Harness::ClaudeCode).label(), "claude-code");
        assert_eq!(adapter_for(Harness::Codex).label(), "codex");
        assert_eq!(adapter_for(Harness::OpenCode).label(), "opencode");
    }

    #[test]
    fn skills_dir_is_harness_native() {
        let root = Path::new("/repo");
        assert_eq!(
            adapter_for(Harness::ClaudeCode).skills_dir(root),
            root.join(".claude").join("skills")
        );
        assert_eq!(
            adapter_for(Harness::Codex).skills_dir(root),
            root.join(".agents").join("skills")
        );
        assert_eq!(
            adapter_for(Harness::OpenCode).skills_dir(root),
            root.join(".opencode").join("skills")
        );
    }

    #[test]
    fn only_codex_and_opencode_rewrite_frontmatter() {
        assert!(!adapter_for(Harness::ClaudeCode).rewrites_frontmatter_name());
        assert!(adapter_for(Harness::Codex).rewrites_frontmatter_name());
        assert!(adapter_for(Harness::OpenCode).rewrites_frontmatter_name());
    }

    #[test]
    fn plan_mode_context_wraps_in_system_reminder_for_every_harness() {
        for h in Harness::ALL {
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
        let claude = adapter_for(Harness::ClaudeCode).run_capabilities();
        assert!(claude.supports_guard);
        assert!(claude.supports_bootstrap_with_no_stage);
        assert!(claude.supports_stage_name_with_no_stage);

        let codex = adapter_for(Harness::Codex).run_capabilities();
        assert!(codex.supports_guard);
        assert!(!codex.supports_bootstrap_with_no_stage);
        assert!(!codex.supports_stage_name_with_no_stage);

        let opencode = adapter_for(Harness::OpenCode).run_capabilities();
        assert!(!opencode.supports_guard);
        assert!(opencode.supports_bootstrap_with_no_stage);
        assert!(opencode.supports_stage_name_with_no_stage);
    }

    #[test]
    fn guard_armed_message_is_harness_specific_and_absent_for_opencode() {
        // The post-arm `--guard` banner names the harness's native hook surface,
        // so it lives behind the adapter rather than in generic run code.
        let claude = adapter_for(Harness::ClaudeCode)
            .guard_armed_message()
            .expect("claude code has a write guard");
        assert!(
            claude.contains(".claude/settings.local.json"),
            "claude banner names its hook file: {claude}"
        );

        let codex = adapter_for(Harness::Codex)
            .guard_armed_message()
            .expect("codex has a write guard");
        assert!(
            codex.contains(".codex/hooks.json"),
            "codex banner names its hook file: {codex}"
        );

        // OpenCode has no write guard (its install_guard errors), so there is no
        // banner to print.
        assert_eq!(adapter_for(Harness::OpenCode).guard_armed_message(), None);
    }

    #[test]
    fn staged_slug_default_and_opencode_override_preserve_the_prefix() {
        let prefix = "slow-powers-eval-";
        assert_eq!(
            adapter_for(Harness::ClaudeCode).staged_slug(prefix, 2, "with_skill", "my-skill"),
            "slow-powers-eval-2-with_skill__my-skill"
        );
        assert_eq!(
            adapter_for(Harness::OpenCode).staged_slug(prefix, 2, "with_skill", "my-skill"),
            "slow-powers-eval-2-with-skill-my-skill"
        );
    }
}
