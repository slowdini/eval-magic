//! Cross-field descriptor invariants — the load-time form of the old
//! cross-harness adapter tests, applied to every descriptor so user-supplied
//! files inherit the same checks as the built-ins.

use regex::Regex;

use crate::adapters::guard;

use super::{
    DescriptorError, GuardEngine, HarnessDescriptor, render_staged_slug, stage_name_error,
};

mod conversation;

/// The placeholders a slug template must carry to keep cleanup prefix-scans
/// and per-cell uniqueness working.
const SLUG_PLACEHOLDERS: [&str; 4] = ["{prefix}", "{iteration}", "{condition}", "{skill_name}"];

/// Check every cross-field invariant, returning the first violation with an
/// actionable message.
pub(super) fn validate_descriptor(
    d: &HarnessDescriptor,
    source: &str,
) -> Result<(), DescriptorError> {
    let fail = |message: String| {
        Err(DescriptorError::Invariant {
            path: source.to_string(),
            message,
        })
    };

    // Guard capability and post-arm banner move in lockstep: `--guard` gates
    // on the capability, and an armed guard the user is never told about (or a
    // banner with no guard behind it) misleads the dispatch session.
    if d.run.supports_guard != d.guard.is_some() {
        return fail(format!(
            "run.supports_guard is {} but the [guard] table is {}; the guard capability and \
             the armed banner must move in lockstep — declare both or neither",
            d.run.supports_guard,
            if d.guard.is_some() {
                "present"
            } else {
                "absent"
            },
        ));
    }

    // Native staging and the write guard both live under the skills dir:
    // staging copies skills into it, and the guard keeps its marker/manifest
    // there. Without a skills_dir neither has anywhere to operate.
    if d.skills_dir.is_none() {
        if d.staging.is_configured() {
            return fail(
                "[staging] is configured but skills_dir is not declared; native staging \
                 copies skills into skills_dir — declare it, or drop [staging] and let \
                 runs fall back to --no-stage"
                    .into(),
            );
        }
        if d.guard.is_some() {
            return fail(
                "[guard] is declared but skills_dir is not; the guard's marker and \
                 teardown manifest live under skills_dir — declare it, or drop the \
                 guard and rely on the detect-stray-writes audit"
                    .into(),
            );
        }
    }

    // Slug shape: one source of truth, all four placeholders, and the
    // generated slug must satisfy the descriptor's own naming rules.
    if d.staging.slug_template.is_some() && d.staging.slug_capability.is_some() {
        return fail(
            "declare either staging.slug_template or staging.slug_capability, not both".into(),
        );
    }
    if let Some(template) = &d.staging.slug_template {
        for placeholder in SLUG_PLACEHOLDERS {
            if !template.contains(placeholder) {
                return fail(format!(
                    "staging.slug_template must contain {placeholder} — cleanup prefix-scans \
                     and per-cell uniqueness rely on all four placeholders"
                ));
            }
        }
    }
    let stage_regex = match &d.staging.stage_name_pattern {
        Some(pattern) => match Regex::new(pattern) {
            Ok(regex) => Some(regex),
            Err(e) => {
                return fail(format!("staging.stage_name_pattern does not compile: {e}"));
            }
        },
        None => None,
    };
    let sample = render_staged_slug(&d.staging, "slow-powers-eval-", 2, "with_skill", "my-skill");
    if !sample.starts_with("slow-powers-eval-") {
        return fail(format!(
            "the staged slug must preserve the prefix (cleanup prefix-scans rely on it); \
             sample slug: \"{sample}\""
        ));
    }
    if let Some(message) = stage_name_error(&d.staging, stage_regex.as_ref(), &sample) {
        return fail(format!(
            "the staged slug \"{sample}\" fails its own stage-name rules ({message}); \
             align staging.slug_template/slug_capability with the naming rules"
        ));
    }

    // The skills dir must live under a declared config dir, or staging's
    // sibling-asset filter would copy a checked-in copy into staged envs.
    if let Some(skills_dir) = &d.skills_dir {
        let top = skills_dir.split('/').next().unwrap_or_default();
        if !d.config_dirs.iter().any(|dir| dir == top) {
            return fail(format!(
                "config_dirs {:?} misses \"{top}\", the parent of skills_dir — staging's \
                 sibling-asset filter and the guard tamper rules key off config_dirs",
                d.config_dirs
            ));
        }
    }

    // The guard block is rendered by the engine at arm/verdict time, and the
    // guard fails open — so every data contract is proven here, before any
    // run arms the hook. Which fields apply depends on the engine: the schema
    // proves per-engine requiredness, and the checks below prove per-engine
    // applicability and content.
    if let Some(guard) = &d.guard {
        let relative_path = |field: &str, path: &str| -> Result<(), DescriptorError> {
            if path.starts_with('/')
                || path
                    .split('/')
                    .any(|seg| seg.is_empty() || seg == "." || seg == "..")
            {
                return fail(format!(
                    "guard.{field} must be a relative `/`-separated path without \".\" or \
                     \"..\" segments (got \"{path}\") — it resolves under the staged env root"
                ));
            }
            Ok(())
        };

        match guard.engine {
            GuardEngine::JsonHooks => {
                if guard.plugin_file.is_some() {
                    return fail(
                        "guard.plugin_file is only valid with engine = \"opencode-plugin\"; \
                         the json-hooks engine merges a hook entry into guard.hooks_file"
                            .into(),
                    );
                }
                let (hooks_file, matcher, command_template, hook_entry) = match (
                    &guard.hooks_file,
                    &guard.matcher,
                    &guard.command_template,
                    &guard.hook_entry,
                ) {
                    (Some(h), Some(m), Some(c), Some(e)) => (h, m, c, e),
                    _ => {
                        return fail(
                            "the json-hooks engine requires hooks_file, matcher, \
                             command_template, and hook_entry (proven by the schema gate)"
                                .into(),
                        );
                    }
                };

                // Every tool the guard hooks must be declared in the vocabulary, or
                // the write-guard arbiter would silently wave it through.
                let vocabulary: Vec<&str> = d
                    .tools
                    .write
                    .iter()
                    .chain(&d.tools.patch)
                    .chain(&d.tools.shell)
                    .map(String::as_str)
                    .collect();
                for token in matcher.split('|') {
                    let token = token.trim_matches(['^', '$']);
                    if !vocabulary.contains(&token) {
                        return fail(format!(
                            "the guard matcher hooks tool \"{token}\" but [tools] does not \
                             declare it in write/patch/shell — the write-guard arbiter would \
                             not recognize it"
                        ));
                    }
                }

                relative_path("hooks_file", hooks_file)?;

                for placeholder in ["{exe}", "{marker}"] {
                    if !command_template.contains(placeholder) {
                        return fail(format!(
                            "guard.command_template must reference {placeholder} — the armed \
                             hook invokes this binary with the marker path"
                        ));
                    }
                }

                match serde_json::from_str::<serde_json::Value>(hook_entry) {
                    Err(e) => {
                        return fail(format!(
                            "guard.hook_entry does not parse as JSON ({e}); it is the hook \
                             object appended to the harness's hook config"
                        ));
                    }
                    Ok(entry) => {
                        if !entry.is_object() {
                            return fail(
                                "guard.hook_entry must be a JSON object — it is appended to \
                                 the hook config's hooks.PreToolUse array"
                                    .into(),
                            );
                        }
                        for placeholder in ["{matcher}", "{command}"] {
                            if !guard::any_string_value_contains(&entry, placeholder) {
                                return fail(format!(
                                    "guard.hook_entry must reference the {placeholder} \
                                     placeholder in a string value — placeholders substitute \
                                     into string values only, so anywhere else would render \
                                     an inert hook"
                                ));
                            }
                        }
                    }
                }
            }
            GuardEngine::OpencodePlugin => {
                // The plugin engine stages the plugin file whole — there is no
                // hook-config merge, tool matcher, or shell command to render.
                for (field, present) in [
                    ("hooks_file", guard.hooks_file.is_some()),
                    ("matcher", guard.matcher.is_some()),
                    ("command_template", guard.command_template.is_some()),
                    ("hook_entry", guard.hook_entry.is_some()),
                ] {
                    if present {
                        return fail(format!(
                            "guard.{field} is only valid with the json-hooks engine; the \
                             opencode-plugin engine stages the plugin file whole — declare \
                             plugin_file instead"
                        ));
                    }
                }
                let Some(plugin_file) = &guard.plugin_file else {
                    return fail(
                        "engine = \"opencode-plugin\" requires plugin_file (proven by the \
                         schema gate)"
                            .into(),
                    );
                };
                relative_path("plugin_file", plugin_file)?;
            }
        }

        match serde_json::from_str::<serde_json::Value>(&guard.verdict_template) {
            Err(e) => {
                return fail(format!(
                    "guard.verdict_template does not parse as JSON ({e}); it is printed \
                     verbatim as the deny verdict"
                ));
            }
            Ok(verdict) => {
                if !guard::any_string_value_contains(&verdict, "{reason}") {
                    return fail(
                        "guard.verdict_template must reference the {reason} placeholder in a \
                         string value — a deny verdict that hides the reason is undebuggable"
                            .into(),
                    );
                }
            }
        }
    }

    // A transcript parser without a write/shell vocabulary makes the
    // stray-writes audit a silent no-op.
    if d.transcript.is_some() && (d.tools.write.is_empty() || d.tools.shell.is_empty()) {
        return fail(
            "[transcript] is declared but [tools] write/shell are empty; \
             detect-stray-writes would audit nothing — declare the harness's tool names"
                .into(),
        );
    }

    // Transcript ingest reads through exactly one tier: a named code parser
    // or the declarative extract block.
    if let Some(transcript) = &d.transcript {
        match (&transcript.parser, &transcript.extract) {
            (Some(_), Some(_)) => {
                return fail(
                    "[transcript] declares both a parser and an extract block; ingest reads \
                     through exactly one — drop one of them. Layered overrides merge fields \
                     and cannot delete an inherited parser, so moving a harness from parser \
                     to extract needs a new label rather than an overlay"
                        .into(),
                );
            }
            (None, None) => {
                return fail(
                    "[transcript] declares neither a parser nor an extract block; declare \
                     exactly one — name a parser capability or add [transcript.extract] — \
                     or drop the table and let llm_judge carry the grading"
                        .into(),
                );
            }
            _ => {}
        }
        if let Some(extract) = &transcript.extract {
            if extract.tools.is_none()
                && extract.final_text.is_none()
                && extract.assistant_messages.is_none()
                && extract.session_id.is_none()
                && extract.tokens.is_none()
                && extract.duration.is_none()
            {
                return fail(
                    "[transcript.extract] declares none of tools/final_text/assistant_messages/\
                     session_id/tokens/duration; an empty extract block parses nothing — \
                     declare at least one output"
                        .into(),
                );
            }
            if let Some(duration) = &extract.duration
                && duration.field.is_some() == duration.timestamp_spread.is_some()
            {
                return fail(
                    "[transcript.extract.duration] must declare exactly one of field \
                     (a millisecond pick) or timestamp_spread (last minus first timestamp)"
                        .into(),
                );
            }
        }
    }

    conversation::validate(d).map_err(|message| DescriptorError::Invariant {
        path: source.to_string(),
        message,
    })?;

    // Tool roles are disjoint: one name in two roles would double-classify
    // invocations in the stray-writes audit.
    let mut seen: Vec<&str> = Vec::new();
    for name in d
        .tools
        .write
        .iter()
        .chain(&d.tools.patch)
        .chain(&d.tools.shell)
        .chain(&d.tools.read)
    {
        if seen.contains(&name.as_str()) {
            return fail(format!(
                "tool \"{name}\" appears in more than one [tools] role — \
                 write/patch/shell/read must be disjoint"
            ));
        }
        seen.push(name);
    }

    // The judge command line splices into the shared judge recipe; its
    // contract (see cli_command::render_judge_dispatch_recipe) is checkable
    // here rather than at render time.
    if let Some(judge) = &d.dispatch.judge_command_template {
        if d.model.is_none() {
            return fail(
                "dispatch.judge_command_template requires model.flag — the judge recipe \
                 splices \"$model_arg\" from each task's model via the model flag"
                    .into(),
            );
        }
        if d.dispatch.capture_prefix.is_none() {
            return fail(
                "dispatch.judge_command_template requires dispatch.capture_prefix — it names \
                 the per-task $response_base capture files"
                    .into(),
            );
        }
        if !judge.contains("$model_arg") {
            return fail(
                "dispatch.judge_command_template must reference $model_arg (empty when a task \
                 declares no model)"
                    .into(),
            );
        }
        if !judge.contains("{cwd}") {
            return fail(
                "dispatch.judge_command_template must contain {cwd} — judges run from the \
                 iteration dir"
                    .into(),
            );
        }
        if !judge.ends_with(" \\") {
            return fail(
                "dispatch.judge_command_template must end with a shell line continuation \
                 (\" \\\") so the recipe's prompt line follows it"
                    .into(),
            );
        }
    }

    // Placeholders must have a backing field, or the template renders with the
    // token left in (the artifact tests' `!contains(\"{{\")` rule, at load time).
    let dispatch = &d.dispatch;
    let pairings: [(&Option<String>, &str, &str, bool); 7] = [
        (
            &dispatch.next_steps_template,
            "next_steps_template",
            "{exec_command}",
            dispatch.exec_template.is_some(),
        ),
        (
            &dispatch.next_steps_template,
            "next_steps_template",
            "{model_note}",
            dispatch.model_note.is_some(),
        ),
        (
            &dispatch.manifest_template,
            "manifest_template",
            "{exec_command}",
            dispatch.exec_template.is_some(),
        ),
        (
            &dispatch.manifest_template,
            "manifest_template",
            "{parallel_recipe}",
            dispatch.parallel_command_template.is_some(),
        ),
        (
            &dispatch.exec_template,
            "exec_template",
            "{guard_args}",
            dispatch.guard_args.is_some(),
        ),
        (
            &dispatch.parallel_command_template,
            "parallel_command_template",
            "{guard_args}",
            dispatch.guard_args.is_some(),
        ),
        (
            &dispatch.judge_command_template,
            "judge_command_template",
            "{guard_args}",
            dispatch.guard_args.is_some(),
        ),
    ];
    for (template, template_name, placeholder, backed) in pairings {
        if template.as_deref().is_some_and(|t| t.contains(placeholder)) && !backed {
            return fail(format!(
                "dispatch.{template_name} references {placeholder} but the field that fills \
                 it is not set"
            ));
        }
    }

    // The manifest template is spliced as `split('\n')` lines; exactly one
    // trailing newline reproduces the section's closing blank line.
    if let Some(manifest) = &d.dispatch.manifest_template
        && (!manifest.ends_with('\n') || manifest.ends_with("\n\n"))
    {
        return fail(
            "dispatch.manifest_template must end with exactly one trailing newline — it \
             becomes the manifest section's closing blank line"
                .into(),
        );
    }

    // A skills-block item that never names the skill renders an unusable list.
    if let Some(block) = &d.skills_block
        && !block.item.contains("{name}")
    {
        return fail("skills_block.item must contain {name}".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::load_descriptor;

    mod conversation;

    /// Load through the full pipeline so each rejection test proves the
    /// invariant fires on a descriptor that already passed the schema gate.
    fn err_of(toml_src: &str) -> String {
        load_descriptor(toml_src, "test.toml")
            .expect_err("descriptor should be rejected")
            .to_string()
    }

    const MINIMAL: &str = r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]
"#;

    /// A guard-wired descriptor whose tool vocabulary covers its matcher; the
    /// base for guard/matcher mutation tests.
    const GUARDED: &str = r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[run]
supports_guard = true

[tools]
write = ["Edit", "MultiEdit", "NotebookEdit", "Write"]
shell = ["Bash"]

[guard]
hooks_file = ".demo/hooks.json"
matcher = "Write|Edit|MultiEdit|NotebookEdit|Bash"
command_template = '"{exe}" guard-hook --harness demo "{marker}"'
hook_entry = '{"matcher":"{matcher}","hooks":[{"type":"command","command":"{command}"}]}'
verdict_template = '{"decision":"block","reason":"{reason}"}'
armed_message = "guard armed"
"#;

    #[test]
    fn rejects_guard_support_without_guard_table() {
        let err = err_of(&format!("{MINIMAL}\n[run]\nsupports_guard = true\n"));
        assert!(err.contains("run.supports_guard"), "{err}");
        assert!(err.contains("lockstep"), "{err}");
    }

    #[test]
    fn rejects_guard_table_without_guard_support() {
        let err = err_of(&GUARDED.replace("supports_guard = true", "supports_guard = false"));
        assert!(err.contains("run.supports_guard"), "{err}");
        assert!(err.contains("lockstep"), "{err}");
    }

    #[test]
    fn rejects_slug_template_missing_a_placeholder() {
        let err = err_of(&format!(
            "{MINIMAL}\n[staging]\nslug_template = \"{{prefix}}{{iteration}}-{{condition}}\"\n"
        ));
        assert!(err.contains("staging.slug_template"), "{err}");
        assert!(err.contains("{skill_name}"), "{err}");
    }

    #[test]
    fn rejects_slug_template_and_capability_together() {
        let err = err_of(&format!(
            "{MINIMAL}\n[staging]\nslug_template = \"{{prefix}}{{iteration}}-{{condition}}__{{skill_name}}\"\nslug_capability = \"opencode\"\n"
        ));
        assert!(err.contains("not both"), "{err}");
    }

    #[test]
    fn rejects_slug_that_fails_its_own_stage_name_rules() {
        // The default slug shape emits `__`, which the single-hyphen pattern
        // rejects — the staged-slug↔naming-rules invariant.
        let err = err_of(&format!(
            "{MINIMAL}\n[staging]\nslug_template = \"{{prefix}}{{iteration}}-{{condition}}__{{skill_name}}\"\nstage_name_pattern = \"^[a-z0-9]+(-[a-z0-9]+)*$\"\nstage_name_max_len = 64\n"
        ));
        assert!(err.contains("stage-name rules"), "{err}");
    }

    #[test]
    fn rejects_config_dirs_missing_skills_dir_parent() {
        let err = err_of(&MINIMAL.replace("[\".demo\"]", "[\".other\"]"));
        assert!(err.contains("parent of skills_dir"), "{err}");
        assert!(err.contains(".demo"), "{err}");
    }

    #[test]
    fn rejects_guard_matcher_tool_missing_from_vocabulary() {
        // The matcher hooks Write|Edit|MultiEdit|NotebookEdit|Bash; drop Bash
        // from the shell vocabulary and the arbiter would wave it through.
        let err = err_of(&GUARDED.replace("shell = [\"Bash\"]", "shell = [\"Shell\"]"));
        assert!(err.contains("Bash"), "{err}");
        assert!(err.contains("[tools]"), "{err}");
    }

    #[test]
    fn rejects_hook_entry_that_is_not_json() {
        let err = err_of(&GUARDED.replace(
            r#"hook_entry = '{"matcher":"{matcher}","hooks":[{"type":"command","command":"{command}"}]}'"#,
            "hook_entry = 'not json'",
        ));
        assert!(err.contains("guard.hook_entry"), "{err}");
        assert!(err.contains("JSON"), "{err}");
    }

    #[test]
    fn rejects_hook_entry_missing_a_placeholder() {
        // {command} in a JSON *key* must not count: only string values are
        // substituted, so a key-side placeholder would render an inert hook.
        for (mutated, needle) in [
            (
                r#"hook_entry = '{"matcher":"{matcher}","hooks":[{"type":"command","{command}":"x"}]}'"#,
                "{command}",
            ),
            (
                r#"hook_entry = '{"matcher":"Write","hooks":[{"type":"command","command":"{command}"}]}'"#,
                "{matcher}",
            ),
        ] {
            let err = err_of(&GUARDED.replace(
                r#"hook_entry = '{"matcher":"{matcher}","hooks":[{"type":"command","command":"{command}"}]}'"#,
                mutated,
            ));
            assert!(err.contains("guard.hook_entry"), "{err}");
            assert!(err.contains(needle), "expected {needle} in: {err}");
        }
    }

    #[test]
    fn rejects_verdict_template_that_is_not_json() {
        let err = err_of(&GUARDED.replace(
            r#"verdict_template = '{"decision":"block","reason":"{reason}"}'"#,
            "verdict_template = 'block it'",
        ));
        assert!(err.contains("guard.verdict_template"), "{err}");
        assert!(err.contains("JSON"), "{err}");
    }

    #[test]
    fn rejects_verdict_template_without_reason_placeholder() {
        let err = err_of(&GUARDED.replace(
            r#"verdict_template = '{"decision":"block","reason":"{reason}"}'"#,
            r#"verdict_template = '{"decision":"block"}'"#,
        ));
        assert!(err.contains("guard.verdict_template"), "{err}");
        assert!(err.contains("{reason}"), "{err}");
    }

    #[test]
    fn rejects_command_template_missing_exe_or_marker() {
        for (mutated, needle) in [
            (
                r#"command_template = 'eval-magic guard-hook "{marker}"'"#,
                "{exe}",
            ),
            (
                r#"command_template = '"{exe}" guard-hook --harness demo'"#,
                "{marker}",
            ),
        ] {
            let err = err_of(&GUARDED.replace(
                r#"command_template = '"{exe}" guard-hook --harness demo "{marker}"'"#,
                mutated,
            ));
            assert!(err.contains("guard.command_template"), "{err}");
            assert!(err.contains(needle), "expected {needle} in: {err}");
        }
    }

    #[test]
    fn rejects_hooks_file_that_escapes_the_env() {
        for mutated in [
            "hooks_file = \"/etc/hooks.json\"",
            "hooks_file = \"../hooks.json\"",
            "hooks_file = \"./hooks.json\"",
        ] {
            let err = err_of(&GUARDED.replace("hooks_file = \".demo/hooks.json\"", mutated));
            assert!(err.contains("guard.hooks_file"), "{err}");
            assert!(err.contains("relative"), "{err}");
        }
    }

    /// A guard wired for the OpenCode plugin engine: the install stages an
    /// embedded JS plugin at `plugin_file` instead of merging a hook entry
    /// into a hook-config file, so the JSON-hooks fields do not apply.
    const PLUGIN_GUARDED: &str = r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[run]
supports_guard = true

[tools]
write = ["edit", "write"]
shell = ["bash"]

[guard]
engine = "opencode-plugin"
plugin_file = ".demo/plugins/eval-guard.js"
verdict_template = '{"decision":"block","reason":"{reason}"}'
armed_message = "guard armed"
"#;

    #[test]
    fn accepts_the_opencode_plugin_engine_shape() {
        load_descriptor(PLUGIN_GUARDED, "test.toml").expect("the plugin engine shape should load");
    }

    #[test]
    fn accepts_an_explicit_json_hooks_engine() {
        let src = GUARDED.replace(
            "hooks_file = \".demo/hooks.json\"",
            "engine = \"json-hooks\"\nhooks_file = \".demo/hooks.json\"",
        );
        load_descriptor(&src, "test.toml").expect("an explicit json-hooks engine should load");
    }

    #[test]
    fn rejects_the_plugin_engine_without_a_plugin_file() {
        let err =
            err_of(&PLUGIN_GUARDED.replace("plugin_file = \".demo/plugins/eval-guard.js\"\n", ""));
        assert!(err.contains("plugin_file"), "{err}");
    }

    #[test]
    fn rejects_a_plugin_file_that_escapes_the_env() {
        for mutated in [
            "plugin_file = \"/etc/eval-guard.js\"",
            "plugin_file = \"../eval-guard.js\"",
            "plugin_file = \"./eval-guard.js\"",
        ] {
            let err = err_of(
                &PLUGIN_GUARDED.replace("plugin_file = \".demo/plugins/eval-guard.js\"", mutated),
            );
            assert!(err.contains("guard.plugin_file"), "{err}");
            assert!(err.contains("relative"), "{err}");
        }
    }

    #[test]
    fn rejects_plugin_engine_with_json_hooks_fields() {
        for (field_line, needle) in [
            ("hooks_file = \".demo/hooks.json\"", "hooks_file"),
            ("matcher = \"write|edit|bash\"", "matcher"),
            (
                "command_template = '\"{exe}\" guard \"{marker}\"'",
                "command_template",
            ),
            ("hook_entry = '{\"matcher\":\"{matcher}\"}'", "hook_entry"),
        ] {
            let src = PLUGIN_GUARDED.replace(
                "engine = \"opencode-plugin\"",
                &format!("engine = \"opencode-plugin\"\n{field_line}"),
            );
            let err = err_of(&src);
            assert!(err.contains(needle), "expected {needle} in: {err}");
            assert!(err.contains("opencode-plugin"), "{err}");
        }
    }

    #[test]
    fn rejects_json_hooks_engine_with_a_plugin_file() {
        // The default engine (no `engine` key) is json-hooks.
        let src = GUARDED.replace(
            "hooks_file = \".demo/hooks.json\"",
            "hooks_file = \".demo/hooks.json\"\nplugin_file = \".demo/plugins/eval-guard.js\"",
        );
        let err = err_of(&src);
        assert!(err.contains("plugin_file"), "{err}");
        assert!(err.contains("json-hooks"), "{err}");
    }

    #[test]
    fn rejects_transcript_without_write_and_shell_tools() {
        let err = err_of(&format!(
            "{MINIMAL}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\nparser = \"codex-items\"\n"
        ));
        assert!(err.contains("detect-stray-writes"), "{err}");
    }

    /// Transcript-shape tests need a tool vocabulary so only the shape rule
    /// under test can fire.
    const TOOLED: &str = r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[tools]
write = ["file_change"]
shell = ["command_execution"]
"#;

    #[test]
    fn rejects_extract_transcript_without_write_and_shell_tools() {
        let err = err_of(&format!(
            "{MINIMAL}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n\
             [transcript.extract.final_text]\nfield = \"text\"\n"
        ));
        assert!(err.contains("detect-stray-writes"), "{err}");
    }

    #[test]
    fn rejects_transcript_with_parser_and_extract() {
        let err = err_of(&format!(
            "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\nparser = \"codex-items\"\n\n\
             [transcript.extract.final_text]\nfield = \"text\"\n"
        ));
        assert!(err.contains("exactly one"), "{err}");
        assert!(err.contains("new label"), "{err}");
    }

    #[test]
    fn rejects_transcript_with_neither_parser_nor_extract() {
        let err = err_of(&format!(
            "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n"
        ));
        assert!(err.contains("exactly one"), "{err}");
        assert!(err.contains("llm_judge"), "{err}");
    }

    #[test]
    fn rejects_empty_extract_block() {
        let err = err_of(&format!(
            "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n[transcript.extract]\n"
        ));
        assert!(err.contains("[transcript.extract]"), "{err}");
        assert!(err.contains("at least one"), "{err}");
    }

    #[test]
    fn rejects_duration_with_both_variants_or_neither() {
        for duration_block in [
            "field = \"elapsed_ms\"\ntimestamp_spread = \"timestamp\"\n",
            "",
        ] {
            let err = err_of(&format!(
                "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n\
                 [transcript.extract.duration]\n{duration_block}"
            ));
            assert!(err.contains("duration"), "{err}");
            assert!(err.contains("exactly one"), "{err}");
        }
    }

    #[test]
    fn rejects_tool_declared_in_more_than_one_role() {
        let err = err_of(&format!(
            "{MINIMAL}\n[tools]\nwrite = [\"Edit\"]\nshell = [\"Edit\"]\n"
        ));
        assert!(err.contains("more than one [tools] role"), "{err}");
        assert!(err.contains("Edit"), "{err}");
    }

    #[test]
    fn rejects_judge_template_without_model_flag() {
        let err = err_of(&format!(
            "{MINIMAL}\n[dispatch]\ncapture_prefix = \"demo\"\njudge_command_template = '    demo --cd \"{{cwd}}\" $model_arg \\'\n"
        ));
        assert!(err.contains("model.flag"), "{err}");
    }

    #[test]
    fn rejects_judge_template_violating_the_recipe_contract() {
        for (template, needle) in [
            ("'    demo --cd \"{cwd}\" \\'", "$model_arg"),
            ("'    demo $model_arg \\'", "{cwd}"),
            ("'    demo --cd \"{cwd}\" $model_arg'", "line continuation"),
        ] {
            let err = err_of(&format!(
                "{MINIMAL}\n[model]\nflag = \"-m\"\n\n[dispatch]\ncapture_prefix = \"demo\"\njudge_command_template = {template}\n"
            ));
            assert!(err.contains(needle), "expected {needle} in: {err}");
        }
    }

    #[test]
    fn rejects_judge_template_without_capture_prefix() {
        let err = err_of(&format!(
            "{MINIMAL}\n[model]\nflag = \"-m\"\n\n[dispatch]\njudge_command_template = '    demo --cd \"{{cwd}}\" $model_arg \\'\n"
        ));
        assert!(err.contains("capture_prefix"), "{err}");
    }

    #[test]
    fn rejects_template_placeholders_without_backing_fields() {
        for (dispatch_body, needle) in [
            (
                "next_steps_template = \"do {exec_command} now\"",
                "{exec_command}",
            ),
            (
                "next_steps_template = \"go.{model_note} then\"",
                "{model_note}",
            ),
            ("exec_template = \"demo{guard_args} run\"", "{guard_args}"),
            (
                "exec_template = \"demo run\"\nmanifest_template = \"use:\\n{exec_command}\\n{parallel_recipe}\\n\"",
                "{parallel_recipe}",
            ),
        ] {
            let err = err_of(&format!("{MINIMAL}\n[dispatch]\n{dispatch_body}\n"));
            assert!(err.contains(needle), "expected {needle} in: {err}");
        }
    }

    #[test]
    fn rejects_manifest_template_without_single_trailing_newline() {
        for manifest in [
            "\"use:\\n{exec_command}\"",
            "\"use:\\n{exec_command}\\n\\n\"",
        ] {
            let err = err_of(&format!(
                "{MINIMAL}\n[dispatch]\nexec_template = \"demo run\"\nmanifest_template = {manifest}\n"
            ));
            assert!(err.contains("exactly one trailing newline"), "{err}");
        }
    }

    #[test]
    fn rejects_skills_block_item_without_name_placeholder() {
        let err = err_of(&format!(
            "{MINIMAL}\n[skills_block]\nheader = \"Skills:\"\nitem = \"- {{description}}\"\n"
        ));
        assert!(err.contains("{name}"), "{err}");
    }

    #[test]
    fn rejects_stage_name_pattern_that_does_not_compile() {
        let err = err_of(&format!(
            "{MINIMAL}\n[staging]\nstage_name_pattern = \"[unclosed\"\n"
        ));
        assert!(err.contains("does not compile"), "{err}");
    }
}
