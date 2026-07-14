//! The harness descriptor registry — the resolution authority for harness
//! identifiers.
//!
//! Descriptor sources load into label-keyed entries, and every way the rest of
//! the crate names a harness funnels through them: [`Harness::resolve`] turns
//! a string into a validated handle (resolving `--harness` after parsing, and
//! behind the artifact `Deserialize`), [`Harness::known`] enumerates the
//! entries, and [`adapter_for`] serves each handle's [`HarnessAdapter`].

use std::path::Path;
use std::sync::{LazyLock, OnceLock};

use crate::core::Harness;

use super::descriptor::layers::{
    DescriptorSource, HarnessFileError, Layer, check_user_layer_restrictions, default_config_root,
    discover_sources, embedded_sources,
};
use super::descriptor::{
    DescriptorError, HarnessDescriptor, finalize_descriptor, merge_descriptor_value,
    parse_descriptor_value,
};
use super::descriptor_adapter::DescriptorAdapter;
use super::harness::{HarnessAdapter, ToolVocabulary};

/// One registry entry: a harness identity (the descriptor's `label`), the
/// descriptor sources that contributed to it (for `harness list`/`show`
/// provenance), the merged descriptor value (the base `harness lint` merges
/// candidate overlays onto), and its descriptor-backed adapter.
#[derive(Debug)]
struct RegistryEntry {
    label: &'static str,
    sources: Vec<(Layer, String)>,
    value: serde_json::Value,
    adapter: DescriptorAdapter,
}

/// Set once by `init_registry` (layered sources), or lazily to the embedded
/// built-ins on first pre-init access. The lazy fallback keeps unit tests
/// hermetic: they never call `init_registry`, so user-supplied descriptor
/// layers can't leak into them.
static REGISTRY: OnceLock<Vec<RegistryEntry>> = OnceLock::new();

fn registry() -> &'static Vec<RegistryEntry> {
    REGISTRY.get_or_init(|| {
        // The embedded descriptors are bundled and known-valid, so a failure
        // here is a programmer error (a bad descriptor edit) and panics —
        // mirroring the bundled-schema panics in `validation::schema` — with
        // the descriptor's own actionable message.
        build_registry(embedded_sources())
            .unwrap_or_else(|e| panic!("bundled harness descriptor is invalid: {e}"))
            .entries
    })
}

/// The registry could not be initialized from the layered sources.
#[derive(Debug, thiserror::Error)]
pub enum RegistryInitError {
    #[error(transparent)]
    HarnessFile(#[from] HarnessFileError),
    #[error(transparent)]
    Build(#[from] RegistryBuildError),
    #[error("the harness registry is already initialized")]
    AlreadyInitialized,
}

/// When `--harness-file` is in play, its descriptor's label — consulted by
/// `Default for Harness` so the one-off harness doesn't have to be repeated
/// via `--harness`.
static SESSION_DEFAULT_HARNESS: OnceLock<&'static str> = OnceLock::new();

/// Initialize the registry from every descriptor layer: embedded built-ins →
/// user-global (`<config-root>/harnesses/*.toml`) → project-local
/// (`<cwd>/.eval-magic/harnesses/*.toml`) → the optional one-off
/// `--harness-file`. Called once by the CLI entry point, before anything
/// resolves a harness.
///
/// Discovered files that fail to load are skipped with a warning on stderr
/// (pointing at `harness lint`); a broken `--harness-file` is a hard error.
/// When `--harness-file` names a descriptor, its label becomes the session's
/// default harness.
pub fn init_registry(harness_file: Option<&Path>) -> Result<(), RegistryInitError> {
    let project_root = std::env::current_dir().unwrap_or_default();
    let (sources, io_warnings) = discover_sources(
        default_config_root().as_deref(),
        &project_root,
        harness_file,
    )?;
    let built = build_registry(sources)?;
    for warning in io_warnings.iter().chain(&built.warnings) {
        eprintln!("⚠ {warning}");
    }
    if harness_file.is_some()
        && let Some(entry) = built
            .entries
            .iter()
            .find(|e| e.sources.iter().any(|(l, _)| *l == Layer::HarnessFile))
    {
        let _ = SESSION_DEFAULT_HARNESS.set(entry.label);
    }
    REGISTRY
        .set(built.entries)
        .map_err(|_| RegistryInitError::AlreadyInitialized)
}

/// A descriptor source set that cannot form a registry. Only embedded and
/// `--harness-file` sources can produce this — discovered layer files fail
/// soft (skipped with a warning in [`BuiltRegistry::warnings`]) so a broken
/// user descriptor never bricks the CLI.
#[derive(Debug, thiserror::Error)]
pub enum RegistryBuildError {
    #[error(transparent)]
    Descriptor(#[from] DescriptorError),
    #[error("duplicate harness label {label:?}: {first} and {second}")]
    DuplicateLabel {
        label: String,
        first: String,
        second: String,
    },
}

/// The outcome of loading a layered source set: the label-keyed entries plus
/// the warnings for every discovered file that was skipped.
#[derive(Debug)]
struct BuiltRegistry {
    entries: Vec<RegistryEntry>,
    warnings: Vec<String>,
}

/// One label's in-progress state while folding layered sources.
struct PendingEntry {
    label: String,
    value: serde_json::Value,
    descriptor: HarnessDescriptor,
    sources: Vec<(Layer, String)>,
}

/// Load descriptor sources into label-keyed entries with layered overrides.
///
/// Sources arrive in precedence order. The first source for a label defines
/// it; a later source with the same label from a *different* layer merges
/// field-by-field on top ([`merge_descriptor_value`]) and the merged result is
/// re-validated with the full provenance chain in its error messages. Two
/// same-label files within one layer have no precedence between them, so the
/// second is rejected (fatal for embedded, a skip-warning otherwise).
///
/// Failure policy per layer: embedded sources are known-valid (a failure is a
/// programmer error, propagated), `--harness-file` was explicitly named (a
/// failure is the user's answer, propagated), and discovered user/project
/// files fail soft — the file is skipped, the message lands in
/// [`BuiltRegistry::warnings`], and any earlier state for that label survives.
fn build_registry(sources: Vec<DescriptorSource>) -> Result<BuiltRegistry, RegistryBuildError> {
    let mut pending: Vec<PendingEntry> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for source in sources {
        let strict = matches!(source.layer, Layer::Embedded | Layer::HarnessFile);
        let mut fail_soft = |e: RegistryBuildError| -> Result<(), RegistryBuildError> {
            if strict {
                Err(e)
            } else {
                warnings.push(format!(
                    "skipping harness descriptor: {e}\n  (run `eval-magic harness lint <file>` \
                     for the full report)"
                ));
                Ok(())
            }
        };

        let value = match parse_descriptor_value(&source.toml_src, &source.path) {
            Ok(value) => value,
            Err(e) => {
                fail_soft(e.into())?;
                continue;
            }
        };
        if source.layer != Layer::Embedded
            && let Err(e) = check_user_layer_restrictions(&value, &source.path)
        {
            fail_soft(e.into())?;
            continue;
        }
        let label = value
            .get("label")
            .and_then(serde_json::Value::as_str)
            .expect("the schema gate requires a string label")
            .to_string();

        match pending.iter().position(|p| p.label == label) {
            None => match finalize_descriptor(&value, &source.path) {
                Ok(descriptor) => pending.push(PendingEntry {
                    label,
                    value,
                    descriptor,
                    sources: vec![(source.layer, source.path)],
                }),
                Err(e) => fail_soft(e.into())?,
            },
            Some(index) => {
                let entry = &mut pending[index];
                let (last_layer, last_path) = entry
                    .sources
                    .last()
                    .expect("every entry records its source");
                if *last_layer == source.layer {
                    fail_soft(RegistryBuildError::DuplicateLabel {
                        label,
                        first: last_path.clone(),
                        second: source.path,
                    })?;
                    continue;
                }
                let mut merged = entry.value.clone();
                merge_descriptor_value(&mut merged, value);
                let provenance = provenance_chain(&entry.sources, &source);
                match finalize_descriptor(&merged, &provenance) {
                    Ok(descriptor) => {
                        entry.value = merged;
                        entry.descriptor = descriptor;
                        entry.sources.push((source.layer, source.path));
                    }
                    Err(e) => fail_soft(e.into())?,
                }
            }
        }
    }

    let entries = pending
        .into_iter()
        .map(|p| RegistryEntry {
            // Leaked once per registry entry per process: the label becomes
            // the `'static` identity the rest of the crate passes around by
            // handle.
            label: Box::leak(p.label.into_boxed_str()),
            sources: p.sources,
            value: p.value,
            adapter: DescriptorAdapter::from_descriptor(p.descriptor),
        })
        .collect();
    Ok(BuiltRegistry { entries, warnings })
}

/// `base.toml (built-in) + overlay.toml (project)` — the provenance string a
/// merged descriptor's validation errors carry.
fn provenance_chain(sources: &[(Layer, String)], next: &DescriptorSource) -> String {
    sources
        .iter()
        .map(|(layer, path)| format!("{path} ({})", layer.display_name()))
        .chain([format!("{} ({})", next.path, next.layer.display_name())])
        .collect::<Vec<_>>()
        .join(" + ")
}

/// The registry-level default harness — what `--harness` falls back to when
/// absent. A registry concept rather than a descriptor field, so layered
/// user-supplied descriptor files (#136) can never fight over an
/// exactly-one-default invariant.
pub const DEFAULT_HARNESS_NAME: &str = "claude-code";

/// A harness name that no registry entry matches. Names every registered
/// harness so the rejection is actionable wherever it surfaces (artifact
/// deserialization, direct resolution).
#[derive(Debug, thiserror::Error)]
#[error("unknown harness '{name}'; known harnesses: {}", known.join(", "))]
pub struct UnknownHarnessError {
    pub name: String,
    pub known: Vec<&'static str>,
}

impl Harness {
    /// Resolve a kebab-case identifier against the descriptor registry — the
    /// only way to obtain a [`Harness`], so every held handle is valid.
    pub fn resolve(name: &str) -> Result<Harness, UnknownHarnessError> {
        registry()
            .iter()
            .find(|e| e.label == name)
            .map(|e| Harness::from_static_name(e.label))
            .ok_or_else(|| UnknownHarnessError {
                name: name.to_string(),
                known: Harness::known().map(Harness::name).collect(),
            })
    }

    /// Every registered harness, in registry (embedded descriptor) order —
    /// for code that must sweep all of them (e.g. guard teardown scans each
    /// harness's skills dir for a marker).
    pub fn known() -> impl Iterator<Item = Harness> {
        registry()
            .iter()
            .map(|e| Harness::from_static_name(e.label))
    }
}

impl Default for Harness {
    fn default() -> Self {
        // A `--harness-file` descriptor becomes the session default, so the
        // one-off harness doesn't have to be repeated via `--harness`.
        Harness::resolve(default_harness_name())
            .expect("the session default resolves against the registry")
    }
}

impl<'de> serde::Deserialize<'de> for Harness {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Harness::resolve(&name).map_err(serde::de::Error::custom)
    }
}

/// Resolve the adapter for a [`Harness`]. This is the single dispatch point on
/// the harness identifier for all harness-specific behavior; every other
/// module goes through the returned trait object.
pub fn adapter_for(harness: Harness) -> &'static dyn HarnessAdapter {
    &registry()
        .iter()
        .find(|e| e.label == harness.name())
        .expect("Harness handles originate from the registry")
        .adapter
}

/// One registered harness as `harness list`/`show`/`lint` see it: identity,
/// contributing sources in layer order, the merged descriptor value, and the
/// resolved descriptor.
pub struct HarnessInfo {
    pub label: &'static str,
    pub sources: &'static [(Layer, String)],
    pub value: &'static serde_json::Value,
    pub descriptor: &'static HarnessDescriptor,
}

/// Every registered harness in registry order, with provenance and the
/// resolved descriptor — the data source for the `harness` subcommands.
pub fn harness_info() -> impl Iterator<Item = HarnessInfo> {
    registry().iter().map(|e| HarnessInfo {
        label: e.label,
        sources: &e.sources,
        value: &e.value,
        descriptor: e.adapter.descriptor(),
    })
}

/// The effective default harness name: the `--harness-file` descriptor's
/// label when one is loaded, else [`DEFAULT_HARNESS_NAME`].
pub fn default_harness_name() -> &'static str {
    SESSION_DEFAULT_HARNESS
        .get()
        .copied()
        .unwrap_or(DEFAULT_HARNESS_NAME)
}

/// The union of every harness's project-local config dir names (sorted,
/// deduplicated): the dirs harness-agnostic code must treat as protected —
/// staging's sibling-asset filter, the guard's Bash tamper rule, and
/// detect-stray-writes' staging-dir lookbehind.
pub fn all_config_dir_names() -> Vec<String> {
    let mut names: Vec<String> = registry()
        .iter()
        .flat_map(|e| e.adapter.config_dir_names())
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
        for entry in registry().iter() {
            let vocab = entry.adapter.tool_vocabulary();
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

    use crate::adapters::descriptor::EMBEDDED_DESCRIPTORS;
    use crate::adapters::descriptor::layers::{Layer, embedded_sources};

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

    fn src(layer: Layer, path: &str, toml_src: &str) -> DescriptorSource {
        DescriptorSource {
            layer,
            path: path.to_string(),
            toml_src: toml_src.to_string(),
        }
    }

    #[test]
    fn duplicate_embedded_label_errors() {
        let mut sources = embedded_sources();
        sources.push(sources[0].clone());
        let err = build_registry(sources).unwrap_err().to_string();
        assert!(err.contains("duplicate harness label"), "{err}");
        assert!(err.contains("claude-code"), "names the label: {err}");
        assert!(
            err.contains("harnesses/claude-code.toml"),
            "names the colliding source files: {err}"
        );
    }

    #[test]
    fn registry_entries_record_embedded_provenance() {
        let built = build_registry(embedded_sources()).unwrap();
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
        assert_eq!(built.entries.len(), EMBEDDED_DESCRIPTORS.len());
        for entry in &built.entries {
            assert_eq!(entry.sources.len(), 1, "one contributing file per built-in");
            assert_eq!(entry.sources[0].0, Layer::Embedded);
            assert!(
                entry.sources[0].1.contains(entry.label),
                "source path names the harness: {}",
                entry.sources[0].1
            );
        }
    }

    #[test]
    fn project_layer_overrides_a_single_field_of_a_builtin() {
        let mut sources = embedded_sources();
        sources.push(src(
            Layer::ProjectLocal,
            ".eval-magic/harnesses/claude-code.toml",
            "label = \"claude-code\"\n\n[model]\nflag = \"--model-x\"\n",
        ));
        let built = build_registry(sources).unwrap();
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
        assert_eq!(built.entries.len(), EMBEDDED_DESCRIPTORS.len());
        let entry = built
            .entries
            .iter()
            .find(|e| e.label == "claude-code")
            .unwrap();
        // The overridden field changed; everything else survives from the
        // embedded base (field-level merge, not file shadowing).
        assert_eq!(entry.adapter.cli_model_flag(), Some("--model-x".into()));
        assert!(entry.adapter.run_capabilities().supports_guard);
        assert_eq!(
            entry.sources,
            vec![
                (Layer::Embedded, "harnesses/claude-code.toml".to_string()),
                (
                    Layer::ProjectLocal,
                    ".eval-magic/harnesses/claude-code.toml".to_string()
                ),
            ]
        );
    }

    #[test]
    fn new_label_in_a_user_layer_registers_a_new_harness() {
        let mut sources = embedded_sources();
        sources.push(src(
            Layer::ProjectLocal,
            ".eval-magic/harnesses/cool.toml",
            "label = \"cool-custom-harness\"\n",
        ));
        let built = build_registry(sources).unwrap();
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
        let entry = built
            .entries
            .iter()
            .find(|e| e.label == "cool-custom-harness")
            .expect("new harness registered");
        assert_eq!(entry.sources[0].0, Layer::ProjectLocal);
        assert!(entry.adapter.skills_dir(Path::new("/r")).is_none());
    }

    #[test]
    fn discovered_file_declaring_a_guard_warns_and_is_skipped() {
        let mut sources = embedded_sources();
        sources.push(src(
            Layer::ProjectLocal,
            ".eval-magic/harnesses/armed.toml",
            "label = \"armed\"\n\n[guard]\nengine = \"claude-hooks\"\narmed_message = \"x\"\n",
        ));
        let built = build_registry(sources).unwrap();
        assert_eq!(
            built.entries.len(),
            EMBEDDED_DESCRIPTORS.len(),
            "file skipped"
        );
        assert_eq!(built.warnings.len(), 1);
        assert!(
            built.warnings[0].contains("may not declare [guard]"),
            "{}",
            built.warnings[0]
        );
    }

    #[test]
    fn discovered_overlay_breaking_invariants_warns_and_keeps_the_base() {
        let mut sources = embedded_sources();
        // Replacing config_dirs orphans skills_dir's parent — an invariant
        // break that only shows post-merge.
        sources.push(src(
            Layer::ProjectLocal,
            ".eval-magic/harnesses/claude-code.toml",
            "label = \"claude-code\"\nconfig_dirs = [\".other\"]\n",
        ));
        let built = build_registry(sources).unwrap();
        assert_eq!(built.warnings.len(), 1);
        assert!(
            built.warnings[0].contains(".eval-magic/harnesses/claude-code.toml"),
            "names the offending file: {}",
            built.warnings[0]
        );
        let entry = built
            .entries
            .iter()
            .find(|e| e.label == "claude-code")
            .unwrap();
        assert_eq!(
            entry.adapter.config_dir_names(),
            vec![".claude".to_string()],
            "embedded base survives the dropped overlay"
        );
        assert_eq!(
            entry.sources.len(),
            1,
            "the bad overlay records no provenance"
        );
    }

    #[test]
    fn same_label_twice_in_one_discovered_layer_warns_and_skips_the_second() {
        let mut sources = embedded_sources();
        sources.push(src(
            Layer::ProjectLocal,
            "a.toml",
            "label = \"claude-code\"\n\n[model]\nflag = \"--from-a\"\n",
        ));
        sources.push(src(
            Layer::ProjectLocal,
            "b.toml",
            "label = \"claude-code\"\n\n[model]\nflag = \"--from-b\"\n",
        ));
        let built = build_registry(sources).unwrap();
        assert_eq!(built.warnings.len(), 1);
        assert!(
            built.warnings[0].contains("a.toml"),
            "{}",
            built.warnings[0]
        );
        assert!(
            built.warnings[0].contains("b.toml"),
            "{}",
            built.warnings[0]
        );
        let entry = built
            .entries
            .iter()
            .find(|e| e.label == "claude-code")
            .unwrap();
        assert_eq!(entry.adapter.cli_model_flag(), Some("--from-a".into()));
    }

    #[test]
    fn harness_file_failures_are_fatal() {
        let mut sources = embedded_sources();
        sources.push(src(Layer::HarnessFile, "one-off.toml", "label = "));
        let err = build_registry(sources).unwrap_err().to_string();
        assert!(err.contains("one-off.toml"), "{err}");
    }

    #[test]
    fn harness_file_guard_rejection_is_fatal() {
        let mut sources = embedded_sources();
        sources.push(src(
            Layer::HarnessFile,
            "one-off.toml",
            "label = \"armed\"\n\n[guard]\nengine = \"claude-hooks\"\narmed_message = \"x\"\n",
        ));
        let err = build_registry(sources).unwrap_err().to_string();
        assert!(err.contains("may not declare [guard]"), "{err}");
    }

    #[test]
    fn harness_file_overlay_merges_on_top_of_discovered_layers() {
        let mut sources = embedded_sources();
        sources.push(src(
            Layer::ProjectLocal,
            "p.toml",
            "label = \"claude-code\"\n\n[model]\nflag = \"--from-project\"\n",
        ));
        sources.push(src(
            Layer::HarnessFile,
            "one-off.toml",
            "label = \"claude-code\"\n\n[model]\nflag = \"--from-file\"\n",
        ));
        let built = build_registry(sources).unwrap();
        assert!(built.warnings.is_empty(), "{:?}", built.warnings);
        let entry = built
            .entries
            .iter()
            .find(|e| e.label == "claude-code")
            .unwrap();
        assert_eq!(entry.adapter.cli_model_flag(), Some("--from-file".into()));
        assert_eq!(entry.sources.len(), 3);
    }

    #[test]
    fn resolve_unknown_name_lists_known_harnesses() {
        let err = Harness::resolve("nonexistent").unwrap_err().to_string();
        assert!(err.contains("unknown harness 'nonexistent'"), "{err}");
        for name in ["claude-code", "codex", "opencode"] {
            assert!(err.contains(name), "error must name {name}: {err}");
        }
    }

    #[test]
    fn resolve_round_trips_every_registry_entry() {
        for harness in Harness::known() {
            assert_eq!(Harness::resolve(harness.name()).unwrap(), harness);
        }
    }

    #[test]
    fn default_harness_is_claude_code() {
        assert_eq!(DEFAULT_HARNESS_NAME, "claude-code");
        assert_eq!(Harness::default().name(), DEFAULT_HARNESS_NAME);
    }

    #[test]
    fn known_iterates_in_descriptor_order() {
        let names: Vec<_> = Harness::known().map(Harness::name).collect();
        assert_eq!(names, ["claude-code", "codex", "opencode"]);
    }

    #[test]
    fn labels_match_kebab_case_identifiers() {
        for name in ["claude-code", "codex", "opencode"] {
            let harness = Harness::resolve(name).unwrap();
            assert_eq!(adapter_for(harness).label(), name);
        }
    }
}
