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

    fn skills_dir(&self, repo_root: &Path) -> PathBuf {
        self.descriptor
            .skills_dir
            .split('/')
            .fold(repo_root.to_path_buf(), |path, segment| path.join(segment))
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
        let Some(template) = &self.descriptor.dispatch.next_steps_template else {
            return String::new();
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
        let template = self.descriptor.dispatch.manifest_template.as_ref()?;
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
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::adapters::descriptor::{EMBEDDED_DESCRIPTORS, load_descriptor};
    use crate::adapters::harness::{
        CliDispatchContext, CliJudgeContext, CliManifestContext, HarnessAdapter, adapter_for,
    };
    use crate::core::{AvailableSkill, Harness};

    fn descriptor_adapter(harness: Harness) -> DescriptorAdapter {
        let index = Harness::ALL
            .iter()
            .position(|&h| h == harness)
            .expect("harness is built in");
        let (source, toml_src) = EMBEDDED_DESCRIPTORS[index];
        DescriptorAdapter::from_descriptor(
            load_descriptor(toml_src, source)
                .unwrap_or_else(|e| panic!("embedded descriptor {source} is invalid: {e}")),
        )
    }

    fn sample_skills() -> Vec<AvailableSkill> {
        vec![
            AvailableSkill {
                name: "zeta-skill".to_string(),
                path: "/x/zeta-skill/SKILL.md".to_string(),
                description: "Does zeta things.".to_string(),
            },
            AvailableSkill {
                name: "alpha-skill".to_string(),
                path: "/x/alpha-skill/SKILL.md".to_string(),
                description: "Does alpha things.".to_string(),
            },
        ]
    }

    /// Transitional equivalence pin for the descriptor cutover: every
    /// observable method of the descriptor-backed adapter matches the legacy
    /// hand-written adapter, across the guard × model context matrix. Deleted
    /// with the legacy adapters once `adapter_for` routes through the
    /// registry.
    #[test]
    fn descriptor_adapter_matches_legacy_adapter_observable_surface() {
        let root = Path::new("/repo");
        let stage_root = Path::new("/stage");
        let iteration_dir = PathBuf::from("/work/iter-1");

        for harness in Harness::ALL {
            let legacy = adapter_for(harness);
            let new = descriptor_adapter(harness);

            assert_eq!(new.label(), legacy.label());
            assert_eq!(new.skills_dir(root), legacy.skills_dir(root));
            assert_eq!(new.run_capabilities(), legacy.run_capabilities());
            assert_eq!(new.config_dir_names(), legacy.config_dir_names());
            assert_eq!(new.tool_vocabulary(), legacy.tool_vocabulary());
            assert_eq!(
                new.rewrites_frontmatter_name(),
                legacy.rewrites_frontmatter_name()
            );
            assert_eq!(
                new.advertises_staged_slug_name(),
                legacy.advertises_staged_slug_name()
            );
            assert_eq!(new.skill_surface_phrase(), legacy.skill_surface_phrase());
            assert_eq!(
                new.skill_unresolved_phrase(),
                legacy.skill_unresolved_phrase()
            );
            assert_eq!(new.cli_events_filename(), legacy.cli_events_filename());
            assert_eq!(new.cli_model_flag(), legacy.cli_model_flag());
            assert_eq!(new.guard_armed_message(), legacy.guard_armed_message());
            assert_eq!(
                new.transcript_surfaces_skill_invocation(),
                legacy.transcript_surfaces_skill_invocation()
            );
            assert_eq!(
                new.guard_hook_cleanup_dir(stage_root),
                legacy.guard_hook_cleanup_dir(stage_root)
            );
            assert_eq!(
                new.render_plan_mode_context("  PLAN\n"),
                legacy.render_plan_mode_context("  PLAN\n")
            );
            assert_eq!(new.render_plan_mode_context(" "), "");

            // Staging: generated slugs and naming-rule verdicts.
            for (prefix, iteration, condition, skill) in [
                ("slow-powers-eval-", 2, "with_skill", "my-skill"),
                ("slow-powers-eval-", 7, "no_skill", "Very_Loud.Skill"),
                (
                    "slow-powers-eval-",
                    1,
                    "with_skill",
                    "a-skill-name-clearly-engineered-to-overflow-opencodes-limit-of-sixty-four",
                ),
            ] {
                assert_eq!(
                    new.staged_slug(prefix, iteration, condition, skill),
                    legacy.staged_slug(prefix, iteration, condition, skill),
                    "{harness:?} slug for {condition}/{skill}"
                );
            }
            for name in [
                "valid-name",
                "slow-powers-eval-2-with-skill-my-skill",
                "Invalid_Name",
                "double--hyphen",
                "-leading",
            ] {
                assert_eq!(
                    new.validate_stage_name(name),
                    legacy.validate_stage_name(name),
                    "{harness:?} stage-name verdict for {name}"
                );
            }

            // Skills block: native shape, sortedness, and the empty guard.
            assert_eq!(
                new.render_available_skills_block(&sample_skills()),
                legacy.render_available_skills_block(&sample_skills())
            );
            assert_eq!(new.render_available_skills_block(&[]), "");

            // Dispatch recipes across the guard × model matrix.
            for guard in [false, true] {
                for agent_model in [None, Some("model-x")] {
                    let ctx = CliDispatchContext {
                        guard,
                        target_args: " --skill-dir /tmp/skills --skill widget-skill",
                        iteration: 2,
                        agent_model,
                    };
                    assert_eq!(
                        new.cli_next_steps(ctx),
                        legacy.cli_next_steps(CliDispatchContext {
                            guard,
                            target_args: " --skill-dir /tmp/skills --skill widget-skill",
                            iteration: 2,
                            agent_model,
                        }),
                        "{harness:?} next steps guard={guard} model={agent_model:?}"
                    );
                    // The manifest consumer joins the lines with '\n'
                    // (build_manifest), so equivalence holds on the joined
                    // text — the descriptor path splits multi-line commands
                    // into more elements than the legacy hand-built vec.
                    assert_eq!(
                        new.cli_manifest_section(CliManifestContext { guard, agent_model })
                            .map(|lines| lines.join("\n")),
                        legacy
                            .cli_manifest_section(CliManifestContext { guard, agent_model })
                            .map(|lines| lines.join("\n")),
                        "{harness:?} manifest guard={guard} model={agent_model:?}"
                    );
                }
                assert_eq!(
                    new.cli_judge_next_steps(CliJudgeContext {
                        guard,
                        iteration_dir: &iteration_dir,
                    }),
                    legacy.cli_judge_next_steps(CliJudgeContext {
                        guard,
                        iteration_dir: &iteration_dir,
                    }),
                    "{harness:?} judge recipe guard={guard}"
                );
            }

            // Unsupported-enhancement errors carry the same kind and message.
            if new.cli_events_filename().is_none() {
                let new_err = new.parse_cli_events(Path::new("/nope")).unwrap_err();
                let legacy_err = legacy.parse_cli_events(Path::new("/nope")).unwrap_err();
                assert_eq!(new_err.kind(), legacy_err.kind());
                assert_eq!(new_err.to_string(), legacy_err.to_string());
                let new_err = new.parse_cli_events_full(Path::new("/nope")).unwrap_err();
                let legacy_err = legacy
                    .parse_cli_events_full(Path::new("/nope"))
                    .unwrap_err();
                assert_eq!(new_err.to_string(), legacy_err.to_string());
            }
            if !new.run_capabilities().supports_guard {
                let new_err = new
                    .install_guard(stage_root, Path::new("/bin/guard"), None)
                    .unwrap_err();
                let legacy_err = legacy
                    .install_guard(stage_root, Path::new("/bin/guard"), None)
                    .unwrap_err();
                assert_eq!(new_err.kind(), legacy_err.kind());
                assert_eq!(new_err.to_string(), legacy_err.to_string());
            }

            // Shadow preflight: only Claude Code wires one; on a bare scan
            // root both sides agree (None for the others by construction).
            if harness != Harness::ClaudeCode {
                assert!(new.detect_shadowed_skills(root, &["x"]).is_none());
                assert!(legacy.detect_shadowed_skills(root, &["x"]).is_none());
            }
        }
    }
}
