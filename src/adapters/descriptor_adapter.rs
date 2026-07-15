//! The generic descriptor-backed [`HarnessAdapter`]: one implementation
//! serving every harness, reading declarative values from a validated
//! [`HarnessDescriptor`] and dispatching code-backed features through the
//! named capabilities in [`super::capabilities`].

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use regex::Regex;

use crate::core::{AvailableSkill, HarnessRunCapabilities, ToolInvocation};

use super::TranscriptSummary;
use super::cli_command::{
    render_cli_model_arg, render_judge_dispatch_recipe, render_parallel_dispatch_recipe,
};
use super::descriptor::{HarnessDescriptor, render_staged_slug, stage_name_error, subst};
use super::harness::{
    CliDispatchContext, CliJudgeContext, CliManifestContext, HarnessAdapter, ToolVocabulary,
};
use super::skill_shadow::PluginShadowReport;
use super::skills_block::{DEFAULT_HEADER, DEFAULT_ITEM, render_skills_block};

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
    fn exec_command(&self, guard: bool, agent_model: Option<&str>) -> String {
        let Some(template) = &self.descriptor.dispatch.exec_template else {
            return String::new();
        };
        let model_arg = render_cli_model_arg(self.model_flag(), agent_model);
        subst(
            template,
            &[
                ("model_arg", &model_arg),
                ("guard_args", self.guard_args(guard)),
            ],
        )
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
            Some(transcript) => transcript.parser.parse(path),
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
            Some(transcript) => transcript.parser.parse_full(path),
            None => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "transcript ingest is not wired for the {} harness",
                    self.label()
                ),
            )),
        }
    }

    fn transcript_surfaces_skill_invocation(&self) -> bool {
        self.descriptor
            .transcript
            .as_ref()
            .is_none_or(|t| t.surfaces_skill_invocation)
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
            Some(guard) => guard.engine.install_guard(stage_root, guard_exe, ttl),
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

    fn guard_hook_cleanup_dir(&self, stage_root: &Path) -> Option<PathBuf> {
        self.descriptor
            .guard
            .as_ref()
            .and_then(|g| g.engine.hook_cleanup_dir(stage_root))
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
                    self.exec_command(ctx.guard, ctx.agent_model)
                ),
                None => format!(
                    "\nNext: read dispatch-manifest.md and dispatch each task through your \
                     harness's one-shot CLI from the task's eval_root, saving the agent's \
                     final reply to outputs/final-message.md.\nThen run `{ingest_line}`."
                ),
            };
        };
        let exec_command = self.exec_command(ctx.guard, ctx.agent_model);
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
            let exec_command = self.exec_command(ctx.guard, ctx.agent_model);
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
        let exec_command = self.exec_command(ctx.guard, ctx.agent_model);
        let parallel_recipe = match &self.descriptor.dispatch.parallel_command_template {
            Some(block_template) => {
                let model_arg = render_cli_model_arg(self.model_flag(), ctx.agent_model);
                render_parallel_dispatch_recipe(&subst(
                    block_template,
                    &[
                        ("model_arg", &model_arg),
                        ("guard_args", self.guard_args(ctx.guard)),
                    ],
                ))
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
            &[("cwd", &cwd), ("guard_args", self.guard_args(ctx.guard))],
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
    use std::path::Path;

    use crate::adapters::harness::{CliDispatchContext, CliJudgeContext};
    use crate::adapters::registry::adapter_for;
    use crate::core::{AvailableSkill, Harness};

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
            })
            .expect("an exec template earns a generic manifest recipe")
            .join("\n");
        assert!(manifest.contains("cool-cli run"), "{manifest}");
        assert!(manifest.contains("final-message.md"), "{manifest}");
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
            });
        assert!(
            !unguarded.contains("--dangerously-bypass-hook-trust"),
            "{unguarded}"
        );
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
                "    codex --ask-for-approval never exec --cd \"/work/iter-1\" --sandbox workspace-write --dangerously-bypass-hook-trust $model_arg --json \\"
            ),
            "{recipe}"
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

```bash
JOBS=${JOBS:-4}
jq -j '.tasks[] | [.dispatch_prompt_path, .response_path, (.model // "")] | @tsv + "\u0000"' judge-tasks.json | \
  xargs -0 -P "$JOBS" -I{} sh -c '
    prompt_path="$(printf "%s" "$1" | cut -f1)"
    response_path="$(printf "%s" "$1" | cut -f2)"
    model="$(printf "%s" "$1" | cut -f3)"
    response_base="${response_path%.json}"
    mkdir -p "$(dirname "$response_path")"
    model_arg=""; [ -n "$model" ] && model_arg="--model $model"
    cd "/work/iter-1" && claude -p --output-format stream-json --verbose --permission-mode acceptEdits $model_arg \
      "Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers." \
      </dev/null \
      > "$response_base.claude-events.jsonl" \
      2> "$response_base.claude-stderr.log"
  ' sh {}
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
