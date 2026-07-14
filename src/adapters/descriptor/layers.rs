//! Descriptor source layers: where a descriptor's TOML text came from.
//!
//! Every descriptor source carries its layer and display path so registry
//! entries can report provenance (`harness list`/`show`) and error messages
//! can name the contributing file.

use super::EMBEDDED_DESCRIPTORS;

/// The layer a descriptor source belongs to, in precedence order (later
/// layers override earlier ones field-by-field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// A built-in descriptor bundled into the binary (`harnesses/*.toml`).
    Embedded,
}

/// One descriptor source: its layer, a display path for error messages and
/// provenance, and the raw TOML text.
#[derive(Debug, Clone)]
pub struct DescriptorSource {
    pub layer: Layer,
    pub path: String,
    pub toml_src: String,
}

/// The bundled built-in descriptors as the registry's base layer.
pub fn embedded_sources() -> Vec<DescriptorSource> {
    EMBEDDED_DESCRIPTORS
        .iter()
        .map(|(path, toml_src)| DescriptorSource {
            layer: Layer::Embedded,
            path: (*path).to_string(),
            toml_src: (*toml_src).to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_sources_cover_every_bundled_descriptor() {
        let sources = embedded_sources();
        assert_eq!(sources.len(), EMBEDDED_DESCRIPTORS.len());
        assert!(sources.iter().all(|s| s.layer == Layer::Embedded));
        assert_eq!(sources[0].path, "harnesses/claude-code.toml");
        assert!(!sources[0].toml_src.is_empty());
    }
}
