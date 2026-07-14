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

pub mod layers;
mod validation;

/// The three built-in harness descriptors, embedded like the schemas: a
/// `(source path, TOML text)` pair per harness, in registry order.
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
    validation::validate_descriptor(&descriptor, source)?;
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

/// The staged-slug shape used when a descriptor declares neither a template
/// nor a capability.
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
        let err = err_of(&MINIMAL.replace("\"demo\"", "\"Not_Kebab\""));
        assert!(err.contains("harness-descriptor schema"), "{err}");
    }

    #[test]
    fn rejects_unknown_guard_engine_name() {
        let err = err_of(&GUARDED.replace("claude-hooks", "mystery-hooks"));
        assert!(err.contains("harness-descriptor schema"), "{err}");
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
