//! Harness descriptor files: the data half of a harness adapter.
//!
//! A descriptor is a TOML file carrying every declarative value a harness
//! adapter exposes (label, dirs, capability booleans, phrases, templates,
//! banner prose) plus references to *named capabilities* — the code-backed
//! features in [`super::capabilities`] (transcript parsers, guard engines,
//! slug generation, shadow preflight).
//!
//! Loading is schema-gated: the TOML transcodes to JSON and must satisfy the
//! bundled `schema/harness-descriptor.schema.json`, then the cross-field
//! invariants in [`validate_descriptor`] — the load-time form of the old
//! cross-harness adapter tests, so user-supplied descriptor files inherit the
//! same checks.

use regex::Regex;
use serde::Deserialize;

use crate::validation::{SchemaName, ValidationError, validate_against_schema};

use super::capabilities::{GuardEngine, ShadowPreflight, SlugCapability, TranscriptParser};

/// The three built-in harness descriptors, embedded like the schemas: a
/// `(source path, TOML text)` pair per harness, in `Harness::ALL` order.
pub const EMBEDDED_DESCRIPTORS: [(&str, &str); 3] = [
    (
        "harnesses/claude-code.toml",
        include_str!("../../harnesses/claude-code.toml"),
    ),
    (
        "harnesses/codex.toml",
        include_str!("../../harnesses/codex.toml"),
    ),
    (
        "harnesses/opencode.toml",
        include_str!("../../harnesses/opencode.toml"),
    ),
];

/// A parsed, schema-checked, invariant-checked harness descriptor.
///
/// Field docs live in `schema/harness-descriptor.schema.json` (the schema gate
/// and this struct are kept honest against each other by
/// [`validate_against_schema`]'s deserialize step).
#[derive(Debug, Clone, Deserialize)]
pub struct HarnessDescriptor {
    pub label: String,
    pub skills_dir: String,
    #[serde(default)]
    pub config_dirs: Vec<String>,
    #[serde(default)]
    pub run: RunSection,
    #[serde(default)]
    pub tools: ToolsSection,
    #[serde(default)]
    pub staging: StagingSection,
    pub skills_block: Option<SkillsBlockSection>,
    pub transcript: Option<TranscriptSection>,
    pub model: Option<ModelSection>,
    pub guard: Option<GuardSection>,
    pub shadow: Option<ShadowSection>,
    #[serde(default)]
    pub dispatch: DispatchSection,
}

/// Run-option capabilities. The `Default` mirrors the baseline every harness
/// gets without opting in: no guard, bootstrap/stage-name allowed unstaged.
#[derive(Debug, Clone, Deserialize)]
pub struct RunSection {
    #[serde(default)]
    pub supports_guard: bool,
    #[serde(default = "default_true")]
    pub supports_bootstrap_with_no_stage: bool,
    #[serde(default = "default_true")]
    pub supports_stage_name_with_no_stage: bool,
}

impl Default for RunSection {
    fn default() -> Self {
        RunSection {
            supports_guard: false,
            supports_bootstrap_with_no_stage: true,
            supports_stage_name_with_no_stage: true,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolsSection {
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub patch: Vec<String>,
    #[serde(default)]
    pub shell: Vec<String>,
    #[serde(default)]
    pub read: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StagingSection {
    pub slug_template: Option<String>,
    pub slug_capability: Option<SlugCapability>,
    pub stage_name_pattern: Option<String>,
    pub stage_name_max_len: Option<usize>,
    pub stage_name_invalid_message: Option<String>,
    #[serde(default)]
    pub rewrites_frontmatter_name: bool,
    #[serde(default)]
    pub advertises_staged_slug_name: bool,
    pub surface_phrase: Option<String>,
    pub unresolved_phrase: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillsBlockSection {
    pub header: String,
    pub item: String,
    #[serde(default)]
    pub footer: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TranscriptSection {
    pub events_filename: String,
    pub parser: TranscriptParser,
    #[serde(default = "default_true")]
    pub surfaces_skill_invocation: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelSection {
    pub flag: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GuardSection {
    pub engine: GuardEngine,
    pub armed_message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShadowSection {
    pub preflight: ShadowPreflight,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DispatchSection {
    pub capture_prefix: Option<String>,
    pub guard_args: Option<String>,
    pub model_note: Option<String>,
    pub next_steps_template: Option<String>,
    pub exec_template: Option<String>,
    pub parallel_command_template: Option<String>,
    pub judge_command_template: Option<String>,
    pub manifest_template: Option<String>,
}

fn default_true() -> bool {
    true
}

/// A descriptor that failed to load. Every variant carries the descriptor's
/// source path so the message is actionable on its own.
#[derive(Debug, thiserror::Error)]
pub enum DescriptorError {
    #[error("{path}: invalid TOML: {message}")]
    Toml { path: String, message: String },
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("{path}: {message}")]
    Invariant { path: String, message: String },
}

/// Parse, schema-check, and invariant-check one descriptor. `source` names the
/// descriptor in error messages (its file path).
pub fn load_descriptor(toml_src: &str, source: &str) -> Result<HarnessDescriptor, DescriptorError> {
    let value: serde_json::Value = toml::from_str(toml_src).map_err(|e| DescriptorError::Toml {
        path: source.to_string(),
        message: e.to_string(),
    })?;
    let descriptor: HarnessDescriptor =
        validate_against_schema(SchemaName::HarnessDescriptor, &value, source)?;
    validate_descriptor(&descriptor, source)?;
    Ok(descriptor)
}

/// Substitute single-brace `{token}` placeholders in `template`.
///
/// Single left-to-right pass over the original template: substituted values
/// are never re-scanned, and unknown or unterminated tokens (shell text like
/// `${JOBS:-4}` or `-I{}`) pass through verbatim.
pub(crate) fn subst(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push('{');
            rest = after;
            continue;
        };
        match vars.iter().find(|(k, _)| *k == &after[..end]) {
            Some((_, value)) => {
                out.push_str(value);
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The staged-slug shape every descriptor must satisfy, and the default when
/// neither a template nor a capability is declared.
const SLUG_PLACEHOLDERS: [&str; 4] = ["{prefix}", "{iteration}", "{condition}", "{skill_name}"];
pub(crate) const DEFAULT_SLUG_TEMPLATE: &str = "{prefix}{iteration}-{condition}__{skill_name}";

/// Render the staged slug for one `(iteration, condition, skill)` cell from
/// the descriptor's slug capability or template (or the default template).
pub(crate) fn render_staged_slug(
    staging: &StagingSection,
    prefix: &str,
    iteration: u32,
    condition: &str,
    skill_name: &str,
) -> String {
    if let Some(capability) = staging.slug_capability {
        return capability.staged_slug(prefix, iteration, condition, skill_name);
    }
    let iteration = iteration.to_string();
    subst(
        staging
            .slug_template
            .as_deref()
            .unwrap_or(DEFAULT_SLUG_TEMPLATE),
        &[
            ("prefix", prefix),
            ("iteration", &iteration),
            ("condition", condition),
            ("skill_name", skill_name),
        ],
    )
}

/// Check `name` against the descriptor's declarative stage-name rules,
/// returning the rejection message on violation.
pub(crate) fn stage_name_error(
    staging: &StagingSection,
    regex: Option<&Regex>,
    name: &str,
) -> Option<String> {
    let len_ok = staging
        .stage_name_max_len
        .is_none_or(|max| name.len() <= max);
    let pattern_ok = regex.is_none_or(|r| r.is_match(name));
    if len_ok && pattern_ok {
        return None;
    }
    Some(match &staging.stage_name_invalid_message {
        Some(message) => subst(message, &[("name", name)]),
        None => format!("stage name \"{name}\" violates the descriptor's naming rules"),
    })
}

/// Cross-field invariants — the load-time form of the old cross-harness
/// adapter tests. Returns the first violation with an actionable message.
fn validate_descriptor(d: &HarnessDescriptor, source: &str) -> Result<(), DescriptorError> {
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
    let top = d.skills_dir.split('/').next().unwrap_or_default();
    if !d.config_dirs.iter().any(|dir| dir == top) {
        return fail(format!(
            "config_dirs {:?} misses \"{top}\", the parent of skills_dir — staging's \
             sibling-asset filter and the guard tamper rules key off config_dirs",
            d.config_dirs
        ));
    }

    // Every tool the guard engine hooks must be declared in the vocabulary,
    // or the write-guard arbiter would silently wave it through.
    if let Some(guard) = &d.guard {
        let vocabulary: Vec<&str> = d
            .tools
            .write
            .iter()
            .chain(&d.tools.patch)
            .chain(&d.tools.shell)
            .map(String::as_str)
            .collect();
        for token in guard.engine.hook_matcher().split('|') {
            let token = token.trim_matches(['^', '$']);
            if !vocabulary.contains(&token) {
                return fail(format!(
                    "the guard engine hooks tool \"{token}\" but [tools] does not declare it \
                     in write/patch/shell — the write-guard arbiter would not recognize it"
                ));
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
    use super::*;

    fn load(toml_src: &str) -> Result<HarnessDescriptor, DescriptorError> {
        load_descriptor(toml_src, "test.toml")
    }

    fn err_of(toml_src: &str) -> String {
        load(toml_src)
            .expect_err("descriptor should be rejected")
            .to_string()
    }

    const MINIMAL: &str = r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]
"#;

    /// A guard-wired descriptor whose tool vocabulary covers the claude-hooks
    /// matcher; the base for guard/matcher mutation tests.
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
engine = "claude-hooks"
armed_message = "guard armed"
"#;

    #[test]
    fn minimal_descriptor_loads() {
        let d = load(MINIMAL).unwrap();
        assert_eq!(d.label, "demo");
        assert_eq!(d.skills_dir, ".demo/skills");
        assert_eq!(d.config_dirs, vec![".demo".to_string()]);
        // Section defaults match the trait defaults.
        assert!(!d.run.supports_guard);
        assert!(d.run.supports_bootstrap_with_no_stage);
        assert!(d.run.supports_stage_name_with_no_stage);
        assert!(!d.staging.rewrites_frontmatter_name);
        assert!(d.guard.is_none());
        assert!(d.transcript.is_none());
    }

    #[test]
    fn guarded_descriptor_loads() {
        let d = load(GUARDED).unwrap();
        assert!(d.run.supports_guard);
        assert!(d.guard.is_some());
    }

    #[test]
    fn embedded_descriptors_load_and_validate() {
        for (source, toml_src) in EMBEDDED_DESCRIPTORS {
            let d = load_descriptor(toml_src, source)
                .unwrap_or_else(|e| panic!("embedded descriptor {source} is invalid: {e}"));
            assert!(!d.label.is_empty());
        }
    }

    #[test]
    fn rejects_invalid_toml_syntax() {
        let err = err_of("label = ");
        assert!(err.contains("test.toml"), "{err}");
        assert!(err.contains("invalid TOML"), "{err}");
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let err = err_of(&format!("{MINIMAL}\nmystery = true\n"));
        assert!(err.contains("harness-descriptor schema"), "{err}");
    }

    #[test]
    fn rejects_non_kebab_case_label() {
        let err = err_of(
            r#"
label = "Not_Kebab"
skills_dir = ".demo/skills"
config_dirs = [".demo"]
"#,
        );
        assert!(err.contains("harness-descriptor schema"), "{err}");
    }

    #[test]
    fn rejects_guard_support_without_guard_table() {
        let err = err_of(
            r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[run]
supports_guard = true
"#,
        );
        assert!(err.contains("run.supports_guard"), "{err}");
        assert!(err.contains("lockstep"), "{err}");
    }

    #[test]
    fn rejects_guard_table_without_guard_support() {
        let err = err_of(
            r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[tools]
write = ["Edit", "MultiEdit", "NotebookEdit", "Write"]
shell = ["Bash"]

[guard]
engine = "claude-hooks"
armed_message = "guard armed"
"#,
        );
        assert!(err.contains("run.supports_guard"), "{err}");
        assert!(err.contains("lockstep"), "{err}");
    }

    #[test]
    fn rejects_unknown_guard_engine_name() {
        let err = err_of(&GUARDED.replace("claude-hooks", "mystery-hooks"));
        assert!(err.contains("harness-descriptor schema"), "{err}");
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
        let err = err_of(
            r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".other"]
"#,
        );
        assert!(err.contains("parent of skills_dir"), "{err}");
        assert!(err.contains(".demo"), "{err}");
    }

    #[test]
    fn rejects_guard_matcher_tool_missing_from_vocabulary() {
        // claude-hooks matches Write|Edit|MultiEdit|NotebookEdit|Bash; drop
        // Bash from the shell vocabulary and the arbiter would wave it through.
        let err = err_of(&GUARDED.replace("shell = [\"Bash\"]", "shell = [\"Shell\"]"));
        assert!(err.contains("Bash"), "{err}");
        assert!(err.contains("[tools]"), "{err}");
    }

    #[test]
    fn rejects_transcript_without_write_and_shell_tools() {
        let err = err_of(&format!(
            "{MINIMAL}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\nparser = \"codex-items\"\n"
        ));
        assert!(err.contains("detect-stray-writes"), "{err}");
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

    #[test]
    fn subst_replaces_tokens_and_passes_unknown_through() {
        let out = subst(
            "run {exec} with ${JOBS:-4} and -I{} on {exec}",
            &[("exec", "demo-cmd")],
        );
        assert_eq!(out, "run demo-cmd with ${JOBS:-4} and -I{} on demo-cmd");
    }

    #[test]
    fn subst_does_not_rescan_substituted_values() {
        let out = subst("{a} {b}", &[("a", "holds-{b}"), ("b", "second")]);
        assert_eq!(out, "holds-{b} second");
    }
}
