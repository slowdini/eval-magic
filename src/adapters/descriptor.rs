//! Harness descriptor files: the data half of a harness adapter.
//!
//! A descriptor is a TOML file carrying every declarative value a harness
//! adapter exposes (label, dirs, capability booleans, phrases, templates,
//! banner prose, the write-guard data block) plus references to *named
//! capabilities* — the code-backed features in [`super::capabilities`]
//! (transcript parsers, slug generation, shadow preflight).
//!
//! Loading is schema-gated: the TOML transcodes to JSON and must satisfy the
//! bundled `schema/harness-descriptor.schema.json`, then the cross-field
//! invariants in [`validate_descriptor`] — the load-time form of the old
//! cross-harness adapter tests, so user-supplied descriptor files inherit the
//! same checks.

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::core::ToolInvocation;
use crate::validation::{SchemaName, ValidationError, validate_against_schema};

use super::capabilities::{ShadowPreflight, SlugCapability, TranscriptParser};
use super::extract::ExtractSpec;
use super::transcript::TranscriptSummary;

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
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HarnessDescriptor {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_dirs: Vec<String>,
    #[serde(default, skip_serializing_if = "RunSection::is_default")]
    pub run: RunSection,
    #[serde(default, skip_serializing_if = "ToolsSection::is_empty")]
    pub tools: ToolsSection,
    #[serde(default, skip_serializing_if = "StagingSection::is_unconfigured")]
    pub staging: StagingSection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills_block: Option<SkillsBlockSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<TranscriptSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard: Option<GuardSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow: Option<ShadowSection>,
    #[serde(default, skip_serializing_if = "DispatchSection::is_empty")]
    pub dispatch: DispatchSection,
}

/// Run-option capabilities. The `Default` mirrors the baseline every harness
/// gets without opting in: no guard, bootstrap/stage-name allowed unstaged.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RunSection {
    #[serde(default)]
    pub supports_guard: bool,
    #[serde(default = "default_true")]
    pub supports_bootstrap_with_no_stage: bool,
    #[serde(default = "default_true")]
    pub supports_stage_name_with_no_stage: bool,
}

impl RunSection {
    /// True when every capability sits at its baseline default — the signal
    /// `harness show` uses to omit the section from authorable output.
    pub fn is_default(&self) -> bool {
        *self == RunSection::default()
    }
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ToolsSection {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub write: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patch: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shell: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<String>,
}

impl ToolsSection {
    /// True when no role declares any tool names.
    pub fn is_empty(&self) -> bool {
        self.write.is_empty()
            && self.patch.is_empty()
            && self.shell.is_empty()
            && self.read.is_empty()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StagingSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug_capability: Option<SlugCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_name_pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_name_max_len: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_name_invalid_message: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub rewrites_frontmatter_name: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub advertises_staged_slug_name: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface_phrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unresolved_phrase: Option<String>,
}

impl StagingSection {
    /// True when any field departs from the defaults — the signal that the
    /// descriptor actually configures native staging.
    pub fn is_configured(&self) -> bool {
        !self.is_unconfigured()
    }

    fn is_unconfigured(&self) -> bool {
        self.slug_template.is_none()
            && self.slug_capability.is_none()
            && self.stage_name_pattern.is_none()
            && self.stage_name_max_len.is_none()
            && self.stage_name_invalid_message.is_none()
            && !self.rewrites_frontmatter_name
            && !self.advertises_staged_slug_name
            && self.surface_phrase.is_none()
            && self.unresolved_phrase.is_none()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillsBlockSection {
    pub header: String,
    pub item: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub footer: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TranscriptSection {
    pub events_filename: String,
    /// Named code parser — for streams that need stitching (keyed joins,
    /// content coercion). Exactly one of `parser`/`extract` is declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parser: Option<TranscriptParser>,
    /// Declarative extractor for flat event streams.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extract: Option<ExtractSpec>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub surfaces_skill_invocation: bool,
    /// Tool name of the deterministic skill-invocation event the
    /// `__skill_invoked` meta-check matches (default `"Skill"` — Claude
    /// Code's Skill tool).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_tool: Option<String>,
    /// Argument of the skill-invocation tool that carries the staged slug
    /// (default `"skill"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_arg: Option<String>,
}

impl TranscriptSection {
    /// Parse the events file into ordered tool invocations, through whichever
    /// tier the descriptor declares.
    pub(crate) fn parse(&self, path: &std::path::Path) -> std::io::Result<Vec<ToolInvocation>> {
        match (&self.parser, &self.extract) {
            (Some(parser), _) => parser.parse(path),
            (None, Some(extract)) => super::extract::parse(extract, path),
            // Validation guarantees one tier is declared; fail like a
            // parser-less harness if a section slips through unchecked.
            (None, None) => Err(unwired_error()),
        }
    }

    /// Parse the events file into a full [`TranscriptSummary`].
    pub(crate) fn parse_full(&self, path: &std::path::Path) -> std::io::Result<TranscriptSummary> {
        match (&self.parser, &self.extract) {
            (Some(parser), _) => parser.parse_full(path),
            (None, Some(extract)) => super::extract::parse_full(extract, path),
            (None, None) => Err(unwired_error()),
        }
    }
}

fn unwired_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "[transcript] declares neither a parser nor an extract block",
    )
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelSection {
    pub flag: String,
}

/// Which install mechanism the guard engine uses: `json-hooks` (the default)
/// merges a rendered hook entry into the harness's hook-config file (Claude
/// Code, Codex); `opencode-plugin` stages an embedded JS plugin whose
/// `tool.execute.before` hook blocks a tool call by throwing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GuardEngine {
    #[default]
    JsonHooks,
    OpencodePlugin,
}

impl GuardEngine {
    /// True at the default — `skip_serializing_if` keeps the discriminator
    /// out of authorable output for the engine every pre-#155 descriptor uses.
    fn is_default(&self) -> bool {
        *self == GuardEngine::default()
    }
}

/// The write-guard data block: everything the guard engine ([`super::guard`])
/// needs to arm the hook surface and render a deny verdict. Which fields apply
/// depends on `engine`: the JSON-hooks four (`hooks_file`, `matcher`,
/// `command_template`, `hook_entry`) are required for `json-hooks` and barred
/// for `opencode-plugin`, which declares `plugin_file` instead — the schema's
/// conditional-required gate plus load-time validation prove the shape, so the
/// engine unwraps with "proven at load". Embedded built-in descriptors only —
/// the guard fails open, so [`layers::check_user_layer_restrictions`] bars user
/// layers from declaring it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuardSection {
    /// Install mechanism; absent means `json-hooks`.
    #[serde(default, skip_serializing_if = "GuardEngine::is_default")]
    pub engine: GuardEngine,
    /// (json-hooks) Hook-config file the entry is merged into, `/`-separated
    /// relative to the staged env root (e.g. `.claude/settings.local.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks_file: Option<String>,
    /// (json-hooks) Tool-name matcher the hook registers for; also the source
    /// of the matcher⊆vocabulary invariant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// (json-hooks) Shell command the hook runs, with `{exe}` and `{marker}`
    /// placeholders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_template: Option<String>,
    /// (json-hooks) JSON template of the hook entry appended to the hook
    /// config's `hooks.PreToolUse` array, with `{matcher}` and `{command}`
    /// placeholders in its string values. Authored key order is serialized
    /// verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_entry: Option<String>,
    /// (opencode-plugin) Plugin file staged from the embedded JS template
    /// (`{exe}`/`{marker}` substituted), `/`-separated relative to the staged
    /// env root (e.g. `.opencode/plugins/slow-powers-eval-guard.js`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_file: Option<String>,
    /// JSON template of the deny verdict printed on stdout, with a `{reason}`
    /// placeholder. Authored key order is the harness's on-disk contract.
    pub verdict_template: String,
    /// Banner printed after `--guard` arms the hook.
    pub armed_message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ShadowSection {
    pub preflight: ShadowPreflight,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DispatchSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard_args: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_steps_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_command_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_command_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_template: Option<String>,
}

impl DispatchSection {
    /// True when no dispatch field is set.
    pub fn is_empty(&self) -> bool {
        self.capture_prefix.is_none()
            && self.guard_args.is_none()
            && self.model_note.is_none()
            && self.next_steps_template.is_none()
            && self.exec_template.is_none()
            && self.parallel_command_template.is_none()
            && self.judge_command_template.is_none()
            && self.manifest_template.is_none()
    }
}

fn default_true() -> bool {
    true
}

/// `skip_serializing_if` helpers for boolean fields at their defaults.
fn is_false(value: &bool) -> bool {
    !*value
}

fn is_true(value: &bool) -> bool {
    *value
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
    finalize_descriptor(&parse_descriptor_value(toml_src, source)?, source)
}

/// Parse one descriptor file's TOML into a JSON value and schema-check it.
/// This is the per-file gate every layered source passes individually — the
/// schema requires only `label`, so partial override files validate here too,
/// while unknown fields and bad capability names are still rejected with the
/// file's own path in the message.
pub fn parse_descriptor_value(
    toml_src: &str,
    source: &str,
) -> Result<serde_json::Value, DescriptorError> {
    let value: serde_json::Value = toml::from_str(toml_src).map_err(|e| DescriptorError::Toml {
        path: source.to_string(),
        message: e.to_string(),
    })?;
    let _: serde_json::Value =
        validate_against_schema(SchemaName::HarnessDescriptor, &value, source)?;
    Ok(value)
}

/// Merge `overlay` onto `base`, field by field: objects merge recursively per
/// key; scalars, arrays, and nulls replace wholesale. This is the layering
/// rule — a later descriptor file overrides individual fields of an earlier
/// one, never whole-file shadowing.
pub fn merge_descriptor_value(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base), serde_json::Value::Object(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(slot) if slot.is_object() && value.is_object() => {
                        merge_descriptor_value(slot, value);
                    }
                    _ => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

/// Deserialize and invariant-check a (possibly merged) descriptor value.
/// `provenance` names every contributing file (e.g. `base.toml (built-in) +
/// overlay.toml (project)`) so a post-merge violation is traceable.
pub fn finalize_descriptor(
    value: &serde_json::Value,
    provenance: &str,
) -> Result<HarnessDescriptor, DescriptorError> {
    let descriptor: HarnessDescriptor =
        validate_against_schema(SchemaName::HarnessDescriptor, value, provenance)?;
    validation::validate_descriptor(&descriptor, provenance)?;
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
hooks_file = ".demo/hooks.json"
matcher = "Write|Edit|MultiEdit|NotebookEdit|Bash"
command_template = '"{exe}" guard-hook --harness demo "{marker}"'
hook_entry = '{"matcher":"{matcher}","hooks":[{"type":"command","command":"{command}"}]}'
verdict_template = '{"decision":"block","reason":"{reason}"}'
armed_message = "guard armed"
"#;

    /// A descriptor whose transcript ingest is the declarative extract tier.
    const EXTRACTED: &str = r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "demo-events.jsonl"

[transcript.extract.tools]
where = { type = "item.completed" }
item = "item"
name_field = "type"
skip_names = ["agent_message"]
args_omit = ["id", "type", "output"]
result_coalesce = ["output"]

[transcript.extract.final_text]
where = { type = "item.completed", "item.type" = "agent_message" }
field = "item.text"

[transcript.extract.tokens]
where = { type = "turn.completed" }
sum = ["usage.input_tokens", "usage.output_tokens"]

[transcript.extract.duration]
timestamp_spread = "timestamp"
"#;

    #[test]
    fn extract_descriptor_loads_and_reserializes_to_loadable_toml() {
        let d = load(EXTRACTED).unwrap();
        let transcript = d.transcript.as_ref().expect("transcript section loads");
        assert!(transcript.parser.is_none());
        assert!(transcript.extract.is_some());

        // `harness show` re-serializes resolved descriptors to TOML; the
        // extract tier must survive the round trip.
        let shown = toml::to_string(&d).expect("descriptor re-serializes");
        let reloaded = load(&shown).unwrap();
        let extract = reloaded.transcript.unwrap().extract.unwrap();
        assert_eq!(
            extract.tokens.unwrap().sum,
            vec!["usage.input_tokens", "usage.output_tokens"]
        );
        assert_eq!(
            extract
                .tools
                .unwrap()
                .r#where
                .get("type")
                .map(String::as_str),
            Some("item.completed")
        );
    }

    #[test]
    fn transcript_section_dispatches_parse_to_the_extract_engine() {
        let transcript = load(EXTRACTED).unwrap().transcript.unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("demo-events.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"ls","output":"ok"}}"#,
                "\n",
                r#"{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"Done."}}"#,
                "\n",
            ),
        )
        .unwrap();

        let invocations = transcript.parse(&path).unwrap();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].name, "command_execution");

        let full = transcript.parse_full(&path).unwrap();
        assert_eq!(full.final_text, Some("Done.".into()));
        assert_eq!(full.tool_invocations.len(), 1);
    }

    #[test]
    fn minimal_descriptor_loads() {
        let d = load(MINIMAL).unwrap();
        assert_eq!(d.label, "demo");
        assert_eq!(d.skills_dir.as_deref(), Some(".demo/skills"));
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
        let guard = d.guard.expect("guard section loads");
        assert_eq!(guard.engine, GuardEngine::JsonHooks);
        assert_eq!(guard.hooks_file.as_deref(), Some(".demo/hooks.json"));
        assert_eq!(
            guard.matcher.as_deref(),
            Some("Write|Edit|MultiEdit|NotebookEdit|Bash")
        );
        assert_eq!(
            guard.command_template.as_deref(),
            Some(r#""{exe}" guard-hook --harness demo "{marker}""#)
        );
        assert_eq!(guard.plugin_file, None);
        assert_eq!(guard.armed_message, "guard armed");
    }

    #[test]
    fn label_only_descriptor_loads_as_baseline() {
        let d = load("label = \"demo\"\n").unwrap();
        assert_eq!(d.label, "demo");
        assert!(d.skills_dir.is_none(), "no skills dir declared");
        assert!(d.config_dirs.is_empty());
        assert!(!d.run.supports_guard);
    }

    #[test]
    fn rejects_staging_without_skills_dir() {
        let err = err_of("label = \"demo\"\n\n[staging]\nsurface_phrase = \"skill\"\n");
        assert!(err.contains("staging"), "{err}");
        assert!(err.contains("skills_dir"), "{err}");
    }

    #[test]
    fn rejects_guard_without_skills_dir() {
        let toml = &GUARDED.replace("skills_dir = \".demo/skills\"\n", "");
        let err = err_of(toml);
        assert!(err.contains("guard"), "{err}");
        assert!(err.contains("skills_dir"), "{err}");
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

    /// Migration canary: the pre-#137 named-engine shape must fail the schema
    /// gate. #155 reintroduced `engine` as a two-variant discriminator
    /// (`json-hooks`/`opencode-plugin`), so the retired `claude-hooks` name
    /// now fails the enum — a stale `engine = "..."` line is still caught
    /// with a schema message instead of silently ignored.
    #[test]
    fn rejects_the_retired_guard_engine_field() {
        let err = err_of(&GUARDED.replace(
            "hooks_file = \".demo/hooks.json\"",
            "engine = \"claude-hooks\"",
        ));
        assert!(err.contains("harness-descriptor schema"), "{err}");
    }

    #[test]
    fn merge_deep_merges_tables_and_replaces_scalars_and_arrays() {
        let mut base: serde_json::Value = toml::from_str(
            r#"
label = "demo"
config_dirs = [".demo"]

[model]
flag = "--model"

[dispatch]
capture_prefix = "demo"
"#,
        )
        .unwrap();
        let overlay: serde_json::Value = toml::from_str(
            r#"
label = "demo"
config_dirs = [".other"]

[model]
flag = "--model-x"
"#,
        )
        .unwrap();
        merge_descriptor_value(&mut base, overlay);
        // Nested-table scalar replaced; sibling table untouched; array replaced
        // wholesale (no element-wise merge).
        assert_eq!(base["model"]["flag"], "--model-x");
        assert_eq!(base["dispatch"]["capture_prefix"], "demo");
        assert_eq!(base["config_dirs"], serde_json::json!([".other"]));
    }

    #[test]
    fn finalize_names_every_contributing_file_on_invariant_breaks() {
        // A merged value violating an invariant reports the provenance chain,
        // not a single path.
        let value: serde_json::Value =
            toml::from_str("label = \"demo\"\nskills_dir = \".demo/skills\"\n").unwrap();
        let err = finalize_descriptor(&value, "base.toml (built-in) + overlay.toml (project)")
            .expect_err("missing config_dirs parent should be rejected")
            .to_string();
        assert!(
            err.contains("base.toml (built-in) + overlay.toml (project)"),
            "{err}"
        );
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
