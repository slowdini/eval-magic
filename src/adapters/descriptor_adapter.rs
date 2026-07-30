//! The generic descriptor-backed [`HarnessAdapter`]: one implementation
//! serving every harness, reading declarative values from a validated
//! [`HarnessDescriptor`] and dispatching code-backed features through the
//! named capabilities in [`super::capabilities`].

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use regex::Regex;

use crate::core::{AvailableSkill, HarnessRunCapabilities, ToolInvocation};
use crate::sandbox::GuardMarker;

use super::cli_command::{
    render_agent_dispatch_command, render_cli_model_arg, render_judge_dispatch_recipe,
    render_parallel_dispatch_recipe,
};
use super::descriptor::{
    HarnessDescriptor, TranscriptSection, render_staged_slug, stage_name_error, subst,
};
use super::harness::{
    CliDispatchContext, CliJudgeContext, CliManifestContext, HarnessAdapter, ToolVocabulary,
};
use super::skill_shadow::PluginShadowReport;
use super::skills_block::{DEFAULT_HEADER, DEFAULT_ITEM, render_skills_block};
use super::{PermissionDenial, TranscriptSummary};

/// A [`HarnessAdapter`] backed by a validated [`HarnessDescriptor`].
#[derive(Debug)]
pub struct DescriptorAdapter {
    descriptor: HarnessDescriptor,
    /// Compiled `staging.stage_name_pattern`; validated to compile at load
    /// time, compiled once here.
    stage_name_regex: Option<Regex>,
}

impl DescriptorAdapter {
    /// Wrap a descriptor that already passed
    /// [`load_descriptor`](super::descriptor::load_descriptor) (which proves
    /// the stage-name pattern compiles).
    pub fn from_descriptor(descriptor: HarnessDescriptor) -> Self {
        let stage_name_regex = descriptor
            .staging
            .stage_name_pattern
            .as_deref()
            .map(|pattern| {
                Regex::new(pattern).expect("stage_name_pattern is validated at descriptor load")
            });
        DescriptorAdapter {
            descriptor,
            stage_name_regex,
        }
    }

    /// The resolved descriptor behind this adapter — the `harness show`/`list`
    /// data source.
    pub(crate) fn descriptor(&self) -> &HarnessDescriptor {
        &self.descriptor
    }

    /// The single-dispatch command: the exec template with `{model_arg}` /
    /// `{guard_args}` filled for this run. Empty when no template is wired.
    fn render_exec_command(
        &self,
        guard: bool,
        agent_model: Option<&str>,
        agent_env: &BTreeMap<String, String>,
    ) -> String {
        let Some(template) = &self.descriptor.dispatch.exec_template else {
            return String::new();
        };
        let model_arg = render_cli_model_arg(self.model_flag(), agent_model);
        render_agent_dispatch_command(
            &subst(
                template,
                &[
                    ("model_arg", &model_arg),
                    ("guard_args", self.guard_args(guard)),
                ],
            ),
            agent_env,
        )
    }

    fn render_resume_command(
        &self,
        guard: bool,
        agent_model: Option<&str>,
        agent_env: &BTreeMap<String, String>,
    ) -> Option<String> {
        let template = &self.descriptor.conversation.as_ref()?.resume_exec_template;
        let model_arg = render_cli_model_arg(self.model_flag(), agent_model);
        Some(render_agent_dispatch_command(
            &subst(
                template,
                &[
                    ("model_arg", &model_arg),
                    ("guard_args", self.guard_args(guard)),
                ],
            ),
            agent_env,
        ))
    }

    /// The `{guard_args}` value for this run: the descriptor's fragment when
    /// the guard is armed, empty otherwise.
    fn guard_args(&self, guard: bool) -> &str {
        if guard {
            self.descriptor.dispatch.guard_args.as_deref().unwrap_or("")
        } else {
            ""
        }
    }

    fn model_flag(&self) -> Option<&str> {
        self.descriptor.model.as_ref().map(|m| m.flag.as_str())
    }
}

impl HarnessAdapter for DescriptorAdapter {
    fn label(&self) -> String {
        self.descriptor.label.clone()
    }

    fn skills_dir(&self, repo_root: &Path) -> Option<PathBuf> {
        self.descriptor.skills_dir.as_ref().map(|dir| {
            dir.split('/')
                .fold(repo_root.to_path_buf(), |path, segment| path.join(segment))
        })
    }

    fn run_capabilities(&self) -> HarnessRunCapabilities {
        HarnessRunCapabilities {
            supports_guard: self.descriptor.run.supports_guard,
            supports_bootstrap_with_no_stage: self.descriptor.run.supports_bootstrap_with_no_stage,
            supports_stage_name_with_no_stage: self
                .descriptor
                .run
                .supports_stage_name_with_no_stage,
        }
    }

    fn config_dir_names(&self) -> Vec<String> {
        self.descriptor.config_dirs.clone()
    }

    fn dispatch_environment(&self) -> BTreeMap<String, String> {
        self.descriptor.dispatch.env.clone()
    }

    fn tool_vocabulary(&self) -> ToolVocabulary {
        ToolVocabulary {
            write_tools: self.descriptor.tools.write.clone(),
            patch_tools: self.descriptor.tools.patch.clone(),
            shell_tools: self.descriptor.tools.shell.clone(),
            read_tools: self.descriptor.tools.read.clone(),
        }
    }

    fn staged_slug(
        &self,
        prefix: &str,
        iteration: u32,
        condition: &str,
        skill_name: &str,
    ) -> String {
        render_staged_slug(
            &self.descriptor.staging,
            prefix,
            iteration,
            condition,
            skill_name,
        )
    }

    fn validate_stage_name(&self, name: &str) -> Result<(), String> {
        match stage_name_error(
            &self.descriptor.staging,
            self.stage_name_regex.as_ref(),
            name,
        ) {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }

    fn rewrites_frontmatter_name(&self) -> bool {
        self.descriptor.staging.rewrites_frontmatter_name
    }

    fn advertises_staged_slug_name(&self) -> bool {
        self.descriptor.staging.advertises_staged_slug_name
    }

    fn render_available_skills_block(&self, skills: &[AvailableSkill]) -> String {
        match &self.descriptor.skills_block {
            Some(block) => render_skills_block(&block.header, &block.item, &block.footer, skills),
            None => render_skills_block(DEFAULT_HEADER, DEFAULT_ITEM, "", skills),
        }
    }

    fn skill_surface_phrase(&self) -> String {
        self.descriptor
            .staging
            .surface_phrase
            .clone()
            .unwrap_or_else(|| "as a discoverable skill".to_string())
    }

    fn skill_unresolved_phrase(&self) -> String {
        self.descriptor
            .staging
            .unresolved_phrase
            .clone()
            .unwrap_or_else(|| "If the staged skill cannot be resolved".to_string())
    }

    fn cli_events_filename(&self) -> Option<String> {
        self.descriptor
            .transcript
            .as_ref()
            .map(|t| t.events_filename.clone())
    }

    fn parse_cli_events(&self, path: &Path) -> io::Result<Vec<ToolInvocation>> {
        match &self.descriptor.transcript {
            Some(transcript) => transcript.parse(path),
            None => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "transcript ingest is not wired for the {} harness",
                    self.label()
                ),
            )),
        }
    }

    fn parse_cli_events_full(&self, path: &Path) -> io::Result<TranscriptSummary> {
        match &self.descriptor.transcript {
            Some(transcript) => transcript.parse_full(path),
            None => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "transcript ingest is not wired for the {} harness",
                    self.label()
                ),
            )),
        }
    }

    fn surfaces_permission_denials(&self) -> bool {
        self.descriptor
            .transcript
            .as_ref()
            .is_some_and(TranscriptSection::surfaces_permission_denials)
    }

    fn parse_permission_denials(&self, path: &Path) -> io::Result<Vec<PermissionDenial>> {
        match &self.descriptor.transcript {
            Some(transcript) => transcript.parse_permission_denials(path),
            // No transcript table: nothing to read, and no error — a harness
            // without ingest simply carries no denial signal.
            None => Ok(Vec::new()),
        }
    }

    fn transcript_skill_invocation(&self) -> Option<(String, String)> {
        match &self.descriptor.transcript {
            Some(t) if !t.surfaces_skill_invocation => None,
            Some(t) => Some((
                t.skill_tool.clone().unwrap_or_else(|| "Skill".to_string()),
                t.skill_arg.clone().unwrap_or_else(|| "skill".to_string()),
            )),
            // No transcript table: the default signature (the meta-check only
            // fires when a run record carries invocations anyway).
            None => Some(("Skill".to_string(), "skill".to_string())),
        }
    }

    fn cli_model_flag(&self) -> Option<String> {
        self.model_flag().map(str::to_string)
    }

    fn install_guard(
        &self,
        stage_root: &Path,
        guard_exe: &Path,
        ttl: Option<Duration>,
    ) -> io::Result<PathBuf> {
        match &self.descriptor.guard {
            Some(guard) => {
                let skills_dir = self
                    .skills_dir(stage_root)
                    .expect("descriptor validation pairs [guard] with skills_dir");
                super::guard::install_guard(guard, &skills_dir, stage_root, guard_exe, ttl)
            }
            None => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("--guard is not supported for the {} harness", self.label()),
            )),
        }
    }

    fn guard_armed_message(&self) -> Option<String> {
        self.descriptor
            .guard
            .as_ref()
            .map(|g| g.armed_message.clone())
    }

    fn guard_verdict(&self, payload: &str, marker: Option<GuardMarker>) -> Option<String> {
        let guard = self.descriptor.guard.as_ref()?;
        super::guard::guard_verdict(guard, &self.label(), payload, marker)
    }

    fn guard_hook_cleanup_dir(&self, stage_root: &Path) -> Option<PathBuf> {
        self.descriptor.guard.as_ref().and_then(|g| {
            super::guard::hook_cleanup_dir(g, self.descriptor.skills_dir.as_deref(), stage_root)
        })
    }

    fn detect_shadowed_skills(
        &self,
        scan_root: &Path,
        staged_skill_names: &[&str],
    ) -> Option<PluginShadowReport> {
        self.descriptor
            .shadow
            .as_ref()
            .and_then(|s| s.preflight.detect(scan_root, staged_skill_names))
    }

    fn isolates_live_sources(&self) -> bool {
        self.descriptor
            .shadow
            .as_ref()
            .is_some_and(|shadow| shadow.isolates_live_sources)
    }

    fn format_shadow_banner(&self, report: &PluginShadowReport) -> String {
        self.descriptor.shadow.as_ref().map_or_else(
            || super::skill_shadow::format_shadow_banner(report),
            |shadow| shadow.preflight.format_banner(report),
        )
    }

    fn shadow_validity_warnings(&self, report: &PluginShadowReport) -> Vec<String> {
        self.descriptor.shadow.as_ref().map_or_else(
            || super::skill_shadow::shadow_validity_warnings(report),
            |shadow| shadow.preflight.validity_warnings(report),
        )
    }

    fn has_dispatch_recipes(&self) -> bool {
        self.descriptor.dispatch.exec_template.is_some()
    }

    fn cli_exec_command(
        &self,
        guard: bool,
        agent_model: Option<&str>,
        agent_env: &BTreeMap<String, String>,
    ) -> Option<String> {
        self.descriptor
            .dispatch
            .exec_template
            .as_ref()
            .map(|_| self.render_exec_command(guard, agent_model, agent_env))
    }

    fn has_conversation_resume(&self) -> bool {
        self.descriptor.conversation.is_some()
    }

    fn conversation_token_usage_aggregation(&self) -> super::TokenUsageAggregation {
        self.descriptor
            .conversation
            .as_ref()
            .map(|conversation| conversation.token_usage_aggregation)
            .unwrap_or_default()
    }

    fn cli_resume_command(
        &self,
        guard: bool,
        agent_model: Option<&str>,
        agent_env: &BTreeMap<String, String>,
    ) -> Option<String> {
        self.render_resume_command(guard, agent_model, agent_env)
    }

    fn cli_next_steps(&self, ctx: CliDispatchContext<'_>) -> String {
        let ingest_line = format!(
            "ingest{} --iteration {} --harness {}",
            ctx.target_args, ctx.iteration, self.descriptor.label
        );
        let Some(template) = &self.descriptor.dispatch.next_steps_template else {
            // Generic fallbacks: a descriptor with just an exec template still
            // earns a copy-pasteable recipe; a baseline descriptor gets the
            // harness-agnostic handoff.
            return match &self.descriptor.dispatch.exec_template {
                Some(_) => format!(
                    "\nNext: iterate the tasks[] array in dispatch.json and dispatch each task \
                     with:\n{}\nThen run `{ingest_line}`.",
                    self.render_exec_command(ctx.guard, ctx.agent_model, ctx.agent_env)
                ),
                None => format!(
                    "\nNext: read dispatch-manifest.md and dispatch each task through your \
                     harness's one-shot CLI from the task's eval_root, saving the agent's \
                     final reply to outputs/final-message.md.\nThen run `{ingest_line}`."
                ),
            };
        };
        let exec_command = self.render_exec_command(ctx.guard, ctx.agent_model, ctx.agent_env);
        let iteration = ctx.iteration.to_string();
        let model_note = if ctx.agent_model.is_some() {
            self.descriptor.dispatch.model_note.as_deref().unwrap_or("")
        } else {
            ""
        };
        subst(
            template,
            &[
                ("exec_command", &exec_command),
                ("target_args", ctx.target_args),
                ("iteration", &iteration),
                ("model_note", model_note),
            ],
        )
    }

    fn cli_manifest_section(&self, ctx: CliManifestContext<'_>) -> Option<Vec<String>> {
        let Some(template) = self.descriptor.dispatch.manifest_template.as_ref() else {
            // An exec template without a manifest template still earns a
            // generic recipe section; without either, the manifest's shared
            // header text already covers the baseline handoff.
            self.descriptor.dispatch.exec_template.as_ref()?;
            let exec_command = self.render_exec_command(ctx.guard, ctx.agent_model, ctx.agent_env);
            return Some(
                format!(
                    "## Dispatch recipe\n\nFrom each task's `eval_root`, dispatch with:\n\
                     {exec_command}\n\nEnsure the agent's final reply lands in the task's \
                     `outputs/final-message.md` (capture it yourself if the command does not \
                     write it).\n"
                )
                .split('\n')
                .map(String::from)
                .collect(),
            );
        };
        let exec_command = self.render_exec_command(ctx.guard, ctx.agent_model, ctx.agent_env);
        let parallel_recipe = match &self.descriptor.dispatch.parallel_command_template {
            Some(block_template) => {
                let model_arg = render_cli_model_arg(self.model_flag(), ctx.agent_model);
                render_parallel_dispatch_recipe(
                    &subst(
                        block_template,
                        &[
                            ("model_arg", &model_arg),
                            ("guard_args", self.guard_args(ctx.guard)),
                        ],
                    ),
                    ctx.one_shot_only,
                    ctx.agent_env,
                )
            }
            None => String::new(),
        };
        Some(
            subst(
                template,
                &[
                    ("exec_command", &exec_command),
                    ("parallel_recipe", &parallel_recipe),
                ],
            )
            .split('\n')
            .map(String::from)
            .collect(),
        )
    }

    fn cli_judge_next_steps(&self, ctx: CliJudgeContext<'_>) -> Option<String> {
        let template = self.descriptor.dispatch.judge_command_template.as_ref()?;
        let cwd = ctx.iteration_dir.display().to_string();
        let command_line = subst(
            template,
            // Judges run from the iteration metadata directory, outside every
            // guarded task env. Hook-trust bypass is only for eval-agent
            // dispatches whose cwd actually contains the vetted guard hook.
            &[("cwd", &cwd), ("guard_args", self.guard_args(false))],
        );
        Some(render_judge_dispatch_recipe(
            &command_line,
            // Both guaranteed by descriptor validation when the template is set.
            self.model_flag().unwrap_or_default(),
            self.descriptor
                .dispatch
                .capture_prefix
                .as_deref()
                .unwrap_or_default(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::LazyLock;

    use crate::adapters::harness::{
        CliDispatchContext, CliJudgeContext, CliManifestContext, TokenUsageAggregation,
    };
    use crate::adapters::registry::adapter_for;
    use crate::core::{AvailableSkill, Harness};

    fn empty_env() -> &'static BTreeMap<String, String> {
        static EMPTY: LazyLock<BTreeMap<String, String>> = LazyLock::new(BTreeMap::new);
        &EMPTY
    }

    fn skill(name: &str, description: &str) -> AvailableSkill {
        AvailableSkill {
            name: name.into(),
            path: format!("/x/{name}/SKILL.md"),
            description: description.into(),
        }
    }

    fn next_steps(harness: Harness, agent_model: Option<&str>) -> String {
        adapter_for(harness).cli_next_steps(CliDispatchContext {
            guard: harness == Harness::resolve("codex").unwrap(),
            target_args: " --skill-dir /tmp/skills --skill widget-skill",
            iteration: 2,
            agent_model,
            agent_env: empty_env(),
        })
    }

    fn adapter_from(toml_src: &str) -> super::DescriptorAdapter {
        super::DescriptorAdapter::from_descriptor(
            crate::adapters::descriptor::load_descriptor(toml_src, "test.toml").unwrap(),
        )
    }

    #[test]
    fn exec_template_alone_yields_generic_dispatch_recipes() {
        use crate::adapters::harness::{CliManifestContext, HarnessAdapter};
        let adapter = adapter_from(
            "label = \"cool-custom-harness\"\n\n[dispatch]\nexec_template = \"cool-cli run <dispatch_prompt_path>{model_arg}\"\n",
        );
        let next = adapter.cli_next_steps(CliDispatchContext {
            guard: false,
            target_args: " --skill-dir /s --skill x",
            iteration: 3,
            agent_model: None,
            agent_env: empty_env(),
        });
        assert!(next.contains("cool-cli run"), "{next}");
        assert!(
            next.contains(
                "ingest --skill-dir /s --skill x --iteration 3 --harness cool-custom-harness"
            ),
            "{next}"
        );
        let manifest = adapter
            .cli_manifest_section(CliManifestContext {
                guard: false,
                agent_model: None,
                agent_env: empty_env(),
                one_shot_only: false,
            })
            .expect("an exec template earns a generic manifest recipe")
            .join("\n");
        assert!(manifest.contains("cool-cli run"), "{manifest}");
        assert!(manifest.contains("final-message.md"), "{manifest}");
    }

    #[test]
    fn descriptor_exposes_declared_live_source_isolation() {
        use crate::adapters::harness::HarnessAdapter;

        let default = adapter_from(
            "label = \"default-shadow\"\n\n[shadow]\npreflight = \"claude-plugins\"\n",
        );
        assert!(!default.isolates_live_sources());

        let isolated = adapter_from(
            "label = \"isolated-shadow\"\n\n[shadow]\npreflight = \"claude-plugins\"\n\
             isolates_live_sources = true\n",
        );
        assert!(isolated.isolates_live_sources());
    }

    #[test]
    fn built_in_harnesses_render_native_resume_commands() {
        for harness in [
            Harness::resolve("claude-code").unwrap(),
            Harness::resolve("codex").unwrap(),
            Harness::resolve("opencode").unwrap(),
        ] {
            let adapter = adapter_for(harness);
            assert!(
                adapter.has_conversation_resume(),
                "{} should support scripted follow-up turns",
                adapter.label()
            );
            let command = adapter
                .cli_resume_command(false, Some("test-model"), empty_env())
                .expect("built-in resume command");
            assert!(command.contains("<eval-root>"), "{command}");
            assert!(command.contains("<outputs_dir>"), "{command}");
            assert!(command.contains("{session_arg}"), "{command}");
            assert!(command.contains("{prompt_arg}"), "{command}");
            assert!(command.contains("test-model"), "{command}");
        }
    }

    #[test]
    fn codex_uses_last_cumulative_conversation_tokens_while_other_builtins_sum() {
        assert_eq!(
            adapter_for(Harness::resolve("codex").unwrap()).conversation_token_usage_aggregation(),
            TokenUsageAggregation::Last
        );
        for name in ["claude-code", "opencode"] {
            assert_eq!(
                adapter_for(Harness::resolve(name).unwrap()).conversation_token_usage_aggregation(),
                TokenUsageAggregation::Sum,
                "{name}"
            );
        }
    }

    #[test]
    fn agent_dispatch_recipes_clear_inherited_git_routing_state() {
        let prelude = "unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY \
                       GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR GIT_CEILING_DIRECTORIES";
        for harness in [
            Harness::resolve("claude-code").unwrap(),
            Harness::resolve("codex").unwrap(),
            Harness::resolve("opencode").unwrap(),
        ] {
            let adapter = adapter_for(harness);
            let exec = adapter.cli_exec_command(false, None, empty_env()).unwrap();
            assert!(exec.starts_with(prelude), "{exec}");
            let resume = adapter
                .cli_resume_command(false, None, empty_env())
                .unwrap();
            assert!(resume.starts_with(prelude), "{resume}");
            let manifest = adapter
                .cli_manifest_section(CliManifestContext {
                    guard: false,
                    agent_model: None,
                    agent_env: empty_env(),
                    one_shot_only: false,
                })
                .unwrap()
                .join("\n");
            assert!(manifest.contains(&format!("    {prelude}\n")), "{manifest}");
        }
    }

    #[test]
    fn baseline_descriptor_yields_the_generic_handoff() {
        use crate::adapters::harness::{CliManifestContext, HarnessAdapter};
        let adapter = adapter_from("label = \"cool-custom-harness\"\n");
        let next = adapter.cli_next_steps(CliDispatchContext {
            guard: false,
            target_args: " --skill x",
            iteration: 1,
            agent_model: None,
            agent_env: empty_env(),
        });
        assert!(next.contains("one-shot CLI"), "{next}");
        assert!(next.contains("outputs/final-message.md"), "{next}");
        assert!(
            next.contains("ingest --skill x --iteration 1 --harness cool-custom-harness"),
            "{next}"
        );
        assert!(
            adapter
                .cli_manifest_section(CliManifestContext {
                    guard: false,
                    agent_model: None,
                    agent_env: empty_env(),
                    one_shot_only: false,
                })
                .is_none(),
            "the manifest's generic header already covers the no-recipe baseline"
        );
    }

    #[test]
    fn exec_recipe_includes_model_only_when_declared() {
        let with = next_steps(Harness::resolve("claude-code").unwrap(), Some("opus"));
        assert!(with.contains("--model opus"), "{with}");
        let without = next_steps(Harness::resolve("claude-code").unwrap(), None);
        assert!(!without.contains("--model "), "{without}");
    }

    #[test]
    fn codex_recipes_gate_hook_trust_on_guard() {
        let guarded = next_steps(Harness::resolve("codex").unwrap(), Some("gpt-5-mini"));
        assert!(
            guarded.contains(
                "codex --ask-for-approval never exec --cd <eval-root> --sandbox workspace-write --dangerously-bypass-hook-trust -m gpt-5-mini --json \\"
            ),
            "{guarded}"
        );
        let unguarded =
            adapter_for(Harness::resolve("codex").unwrap()).cli_next_steps(CliDispatchContext {
                guard: false,
                target_args: "",
                iteration: 2,
                agent_model: None,
                agent_env: empty_env(),
            });
        assert!(
            !unguarded.contains("--dangerously-bypass-hook-trust"),
            "{unguarded}"
        );
    }

    #[test]
    fn opencode_exec_recipe_carries_dir_auto_and_the_model_flag() {
        let with = next_steps(
            Harness::resolve("opencode").unwrap(),
            Some("opencode/gpt-5-nano"),
        );
        assert!(
            with.contains(
                "opencode run --dir <eval-root> --format json --auto -m opencode/gpt-5-nano \\"
            ),
            "{with}"
        );
        let without = next_steps(Harness::resolve("opencode").unwrap(), None);
        assert!(
            without.contains("opencode run --dir <eval-root> --format json --auto \\"),
            "{without}"
        );
        assert!(!without.contains(" -m "), "{without}");
    }

    #[test]
    fn codex_judge_recipe_splices_model_arg_in_one_command_shape() {
        let recipe = adapter_for(Harness::resolve("codex").unwrap())
            .cli_judge_next_steps(CliJudgeContext {
                guard: true,
                iteration_dir: Path::new("/work/iter-1"),
            })
            .expect("codex judge recipe is wired");
        // One command shape: the optional model flag is spliced via $model_arg
        // (same structure as the Claude judge recipe), not an if/else pair.
        assert!(
            recipe.contains(
                "    codex --ask-for-approval never exec --cd \"/work/iter-1\" --sandbox workspace-write $model_arg --json \\"
            ),
            "{recipe}"
        );
        assert!(
            !recipe.contains("--dangerously-bypass-hook-trust"),
            "judges run outside guarded task envs: {recipe}"
        );
        assert!(
            recipe.contains("    model_arg=\"\"; [ -n \"$model\" ] && model_arg=\"-m $model\""),
            "{recipe}"
        );
        assert!(!recipe.contains("if [ -n"), "{recipe}");
    }

    #[test]
    fn claude_judge_recipe_snapshot_is_stable() {
        // Full-string pin carried over from the pre-descriptor adapter: locks
        // the Claude judge recipe byte-for-byte through the descriptor path.
        let recipe = adapter_for(Harness::resolve("claude-code").unwrap())
            .cli_judge_next_steps(CliJudgeContext {
                guard: false,
                iteration_dir: Path::new("/work/iter-1"),
            })
            .expect("claude judge recipe is wired");
        let expected = r#"Dispatch each judge task from judge-tasks.json with:
Existing nonempty response files are skipped; delete one to dispatch that judge again.

```bash
JOBS=${JOBS:-4}
jq -r '.tasks[] | .dispatch_prompt_path, .response_path, ("model=" + (.model // ""))' judge-tasks.json \
  | tr '\n' '\0' \
  | xargs -0 -P "$JOBS" -n 3 sh -c '
    prompt_path="$1"
    response_path="$2"
    model="${3#model=}"
    if [ -s "$response_path" ]; then exit 0; fi
    response_base="${response_path%.json}"
    mkdir -p "$(dirname "$response_path")"
    model_arg=""; [ -n "$model" ] && model_arg="--model $model"
    cd "/work/iter-1" && claude -p --output-format stream-json --verbose --permission-mode bypassPermissions $model_arg \
      "Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers." \
      </dev/null \
      > "$response_base.claude-events.jsonl" \
      2> "$response_base.claude-stderr.log"
  ' sh
```"#;
        assert_eq!(recipe, expected);
    }

    #[test]
    fn skills_blocks_render_each_harness_native_shape() {
        let skills = vec![skill("zebra", "z skill"), skill("alpha", "a skill")];

        let claude = adapter_for(Harness::resolve("claude-code").unwrap())
            .render_available_skills_block(&skills);
        assert!(
            claude.starts_with("The following skills are available for use with the Skill tool:"),
            "{claude}"
        );
        assert!(claude.contains("\n- alpha: a skill"), "{claude}");

        let codex =
            adapter_for(Harness::resolve("codex").unwrap()).render_available_skills_block(&skills);
        assert!(codex.starts_with("## Skills"), "{codex}");
        assert!(
            codex.contains("- alpha: a skill (file: /x/alpha/SKILL.md)"),
            "{codex}"
        );

        let opencode = adapter_for(Harness::resolve("opencode").unwrap())
            .render_available_skills_block(&skills);
        assert!(opencode.starts_with("<available_skills>"), "{opencode}");
        assert!(opencode.ends_with("\n</available_skills>"), "{opencode}");
        assert!(opencode.contains("<name>alpha</name>"), "{opencode}");
        assert!(
            opencode.contains("<description>a skill</description>"),
            "{opencode}"
        );

        // Sorted by name in every shape, and empty renders empty.
        for harness in Harness::known() {
            let block = adapter_for(harness).render_available_skills_block(&skills);
            assert!(
                block.find("alpha").unwrap() < block.find("zebra").unwrap(),
                "{harness:?} sorts by name: {block}"
            );
            assert_eq!(adapter_for(harness).render_available_skills_block(&[]), "");
        }
    }

    #[test]
    fn opencode_stage_name_rules_match_the_old_validator() {
        let adapter = adapter_for(Harness::resolve("opencode").unwrap());
        assert!(adapter.validate_stage_name("valid-name-2").is_ok());
        for invalid in [
            "Invalid_Name",
            "double--hyphen",
            "-leading",
            "trailing-",
            "",
        ] {
            let err = adapter
                .validate_stage_name(invalid)
                .expect_err("name should be rejected");
            assert!(
                err.contains(&format!("OpenCode skill name \"{invalid}\" is invalid")),
                "{err}"
            );
        }
        assert!(adapter.validate_stage_name(&"a".repeat(64)).is_ok());
        assert!(adapter.validate_stage_name(&"a".repeat(65)).is_err());
    }
}
