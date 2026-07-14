//! The harness descriptor registry — the resolution authority for harness
//! identifiers.
//!
//! Descriptor sources load into label-keyed entries, and every way the rest of
//! the crate names a harness funnels through them: [`Harness::resolve`] turns
//! a string into a validated handle (also behind the `--harness` value parser
//! and the artifact `Deserialize`), [`Harness::known`] enumerates the entries,
//! and [`adapter_for`] serves each handle's [`HarnessAdapter`].

use std::sync::{LazyLock, OnceLock};

use crate::core::Harness;

use super::descriptor::layers::{DescriptorSource, Layer, embedded_sources};
use super::descriptor::{DescriptorError, load_descriptor};
use super::descriptor_adapter::DescriptorAdapter;
use super::harness::{HarnessAdapter, ToolVocabulary};

/// One registry entry: a harness identity (the descriptor's `label`), the
/// descriptor sources that contributed to it (for `harness list`/`show`
/// provenance), and its descriptor-backed adapter.
#[derive(Debug)]
struct RegistryEntry {
    label: &'static str,
    sources: Vec<(Layer, String)>,
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
    })
}

/// A descriptor source set that cannot form a registry.
#[derive(Debug, thiserror::Error)]
enum RegistryBuildError {
    #[error(transparent)]
    Descriptor(#[from] DescriptorError),
    #[error("duplicate harness label {label:?}: {first} and {second}")]
    DuplicateLabel {
        label: String,
        first: String,
        second: String,
    },
}

/// Load descriptor sources into registry entries, keyed by each descriptor's
/// `label`. The label is the harness identity — `--harness <label>`, artifact
/// values, and adapter lookup all resolve through it — so two sources
/// producing the same label collide.
fn build_registry(
    sources: Vec<DescriptorSource>,
) -> Result<Vec<RegistryEntry>, RegistryBuildError> {
    let mut entries: Vec<RegistryEntry> = Vec::new();
    for source in sources {
        let descriptor = load_descriptor(&source.toml_src, &source.path)?;
        if let Some(existing) = entries.iter().find(|e| e.label == descriptor.label) {
            return Err(RegistryBuildError::DuplicateLabel {
                label: descriptor.label,
                first: existing
                    .sources
                    .last()
                    .expect("every entry records its source")
                    .1
                    .clone(),
                second: source.path,
            });
        }
        // Leaked once per registry entry per process: the label becomes the
        // `'static` identity the rest of the crate passes around by handle.
        let label: &'static str = Box::leak(descriptor.label.clone().into_boxed_str());
        entries.push(RegistryEntry {
            label,
            sources: vec![(source.layer, source.path)],
            adapter: DescriptorAdapter::from_descriptor(descriptor),
        });
    }
    Ok(entries)
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
        Harness::resolve(DEFAULT_HARNESS_NAME)
            .expect("the embedded registry contains the default harness")
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

    #[test]
    fn duplicate_label_in_same_layer_errors() {
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
        let entries = build_registry(embedded_sources()).unwrap();
        assert_eq!(entries.len(), EMBEDDED_DESCRIPTORS.len());
        for entry in &entries {
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
