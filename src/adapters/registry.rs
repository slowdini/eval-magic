//! The harness descriptor registry — the resolution authority for harness
//! identifiers.
//!
//! Descriptor sources load into label-keyed entries, and every way the rest of
//! the crate names a harness funnels through them: [`Harness::resolve`] turns
//! a string into a validated handle (also behind the `--harness` value parser
//! and the artifact `Deserialize`), [`Harness::known`] enumerates the entries,
//! and [`adapter_for`] serves each handle's [`HarnessAdapter`].

use std::sync::LazyLock;

use crate::core::Harness;

use super::descriptor::{EMBEDDED_DESCRIPTORS, load_descriptor};
use super::descriptor_adapter::DescriptorAdapter;
use super::harness::{HarnessAdapter, ToolVocabulary};

/// One registry entry: a harness identity (the descriptor's `label`) and its
/// descriptor-backed adapter.
struct RegistryEntry {
    label: &'static str,
    adapter: DescriptorAdapter,
}

/// Built on first use. The embedded descriptors are bundled and known-valid,
/// so a load failure here is a programmer error (a bad descriptor edit) and
/// panics — mirroring the bundled-schema panics in `validation::schema` —
/// with the descriptor's own actionable message.
static REGISTRY: LazyLock<Vec<RegistryEntry>> =
    LazyLock::new(|| build_registry(EMBEDDED_DESCRIPTORS));

/// Load descriptor sources into registry entries, keyed by each descriptor's
/// `label`. The label is the harness identity — `--harness <label>`, artifact
/// values, and adapter lookup all resolve through it — so a duplicate is a
/// programmer error and panics. Layering user-supplied descriptor files
/// (#136) means feeding more sources through here.
fn build_registry(
    sources: impl IntoIterator<Item = (&'static str, &'static str)>,
) -> Vec<RegistryEntry> {
    let mut entries: Vec<RegistryEntry> = Vec::new();
    for (source, toml_src) in sources {
        let descriptor = load_descriptor(toml_src, source)
            .unwrap_or_else(|e| panic!("bundled harness descriptor is invalid: {e}"));
        // Leaked once per registry entry per process: the label becomes the
        // `'static` identity the rest of the crate passes around by handle.
        let label: &'static str = Box::leak(descriptor.label.clone().into_boxed_str());
        assert!(
            !entries.iter().any(|e| e.label == label),
            "duplicate harness label {label:?} (from {source})"
        );
        entries.push(RegistryEntry {
            label,
            adapter: DescriptorAdapter::from_descriptor(descriptor),
        });
    }
    entries
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
        REGISTRY
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
        REGISTRY.iter().map(|e| Harness::from_static_name(e.label))
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

/// The `--harness` value parser: registry-driven possible values, so clap
/// renders `[possible values: …]` in help, suggests near-misses, and rejects
/// unknown names listing the known ones — with no compile-time harness set.
pub fn harness_value_parser() -> clap::builder::ValueParser {
    use clap::builder::TypedValueParser as _;
    let names: Vec<&'static str> = Harness::known().map(Harness::name).collect();
    clap::builder::PossibleValuesParser::new(names)
        .map(|name| Harness::resolve(&name).expect("clap only passes registry-known names"))
        .into()
}

/// Resolve the adapter for a [`Harness`]. This is the single dispatch point on
/// the harness identifier for all harness-specific behavior; every other
/// module goes through the returned trait object.
pub fn adapter_for(harness: Harness) -> &'static dyn HarnessAdapter {
    &REGISTRY
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
    let mut names: Vec<String> = REGISTRY
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
        for entry in REGISTRY.iter() {
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
    #[should_panic(expected = "duplicate harness label")]
    fn duplicate_label_panics() {
        build_registry([EMBEDDED_DESCRIPTORS[0], EMBEDDED_DESCRIPTORS[0]]);
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
