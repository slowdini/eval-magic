//! Descriptor source layers: where a descriptor's TOML text came from.
//!
//! Discovery order is precedence order — embedded built-ins, then user-global
//! (`~/.config/eval-magic/harnesses/*.toml`), then project-local
//! (`.eval-magic/harnesses/*.toml`), then a one-off `--harness-file`. Later
//! layers override earlier ones field-by-field (see
//! [`merge_descriptor_value`](super::merge_descriptor_value)). Every source
//! carries its layer and display path so registry entries can report
//! provenance (`harness list`/`show`) and error messages can name the
//! contributing file.

use std::fs;
use std::path::{Path, PathBuf};

use super::{DescriptorError, EMBEDDED_DESCRIPTORS};

/// The layer a descriptor source belongs to, in precedence order (later
/// layers override earlier ones field-by-field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// A built-in descriptor bundled into the binary (`harnesses/*.toml`).
    Embedded,
    /// A user-global descriptor (`<config-root>/harnesses/*.toml`, where the
    /// config root is `$EVAL_MAGIC_CONFIG_DIR`, `$XDG_CONFIG_HOME/eval-magic`,
    /// or `~/.config/eval-magic`).
    UserGlobal,
    /// A project-local descriptor (`<cwd>/.eval-magic/harnesses/*.toml`).
    ProjectLocal,
    /// A one-off descriptor passed via `--harness-file <path>`.
    HarnessFile,
}

impl Layer {
    /// The short name `harness list`/`show` and warnings display.
    pub fn display_name(self) -> &'static str {
        match self {
            Layer::Embedded => "built-in",
            Layer::UserGlobal => "user",
            Layer::ProjectLocal => "project",
            Layer::HarnessFile => "file",
        }
    }
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

/// A `--harness-file` path that could not be read. Unlike discovered layer
/// files (skipped with a warning), the explicitly named file is a hard error.
#[derive(Debug, thiserror::Error)]
#[error("cannot read --harness-file {path}: {message}")]
pub struct HarnessFileError {
    pub path: String,
    pub message: String,
}

/// Discover every descriptor source in precedence order: embedded →
/// user-global → project-local → `--harness-file`. Missing layer directories
/// are fine (empty layer); unreadable files inside a layer directory are
/// skipped and reported in the returned warnings. Only the explicit
/// `--harness-file` fails hard.
pub fn discover_sources(
    config_root: Option<&Path>,
    project_root: &Path,
    harness_file: Option<&Path>,
) -> Result<(Vec<DescriptorSource>, Vec<String>), HarnessFileError> {
    let mut warnings = Vec::new();
    let mut sources = embedded_sources();
    if let Some(config_root) = config_root {
        sources.extend(dir_sources(
            &config_root.join("harnesses"),
            Layer::UserGlobal,
            &mut warnings,
        ));
    }
    sources.extend(dir_sources(
        &project_root.join(".eval-magic").join("harnesses"),
        Layer::ProjectLocal,
        &mut warnings,
    ));
    if let Some(file) = harness_file {
        let toml_src = fs::read_to_string(file).map_err(|e| HarnessFileError {
            path: file.display().to_string(),
            message: e.to_string(),
        })?;
        sources.push(DescriptorSource {
            layer: Layer::HarnessFile,
            path: file.display().to_string(),
            toml_src,
        });
    }
    Ok((sources, warnings))
}

/// Every `*.toml` in `dir`, sorted by filename for deterministic layering.
fn dir_sources(dir: &Path, layer: Layer, warnings: &mut Vec<String>) -> Vec<DescriptorSource> {
    let Ok(entries) = fs::read_dir(dir) else {
        // No directory means an empty layer, not an error.
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "toml") && p.is_file())
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| match fs::read_to_string(&path) {
            Ok(toml_src) => Some(DescriptorSource {
                layer,
                path: path.display().to_string(),
                toml_src,
            }),
            Err(e) => {
                warnings.push(format!(
                    "skipping unreadable harness descriptor {}: {e}",
                    path.display()
                ));
                None
            }
        })
        .collect()
}

/// Resolve the user-global config root from explicit/env inputs:
/// `$EVAL_MAGIC_CONFIG_DIR` verbatim (empty disables the layer), else
/// `$XDG_CONFIG_HOME/eval-magic`, else `<home>/.config/eval-magic`.
pub fn config_root_from(
    explicit: Option<&str>,
    xdg_config_home: Option<&str>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(explicit) = explicit {
        if explicit.is_empty() {
            return None;
        }
        return Some(PathBuf::from(explicit));
    }
    if let Some(xdg) = xdg_config_home
        && !xdg.is_empty()
    {
        return Some(Path::new(xdg).join("eval-magic"));
    }
    home.map(|home| home.join(".config").join("eval-magic"))
}

/// [`config_root_from`] over the live environment.
pub fn default_config_root() -> Option<PathBuf> {
    config_root_from(
        std::env::var("EVAL_MAGIC_CONFIG_DIR").ok().as_deref(),
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::home_dir().as_deref(),
    )
}

/// Reject descriptor content user-supplied files may not declare: the write
/// guard installs native hook config into dispatch environments, so it stays
/// restricted to built-in descriptors until the guard engine is opened up.
/// The check runs on each file's own content, so a user overlay of a guarded
/// built-in still inherits the embedded guard cleanly.
pub fn check_user_layer_restrictions(
    value: &serde_json::Value,
    path: &str,
) -> Result<(), DescriptorError> {
    let declares_guard = value.get("guard").is_some();
    let flips_support = value
        .pointer("/run/supports_guard")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    if declares_guard || flips_support {
        return Err(DescriptorError::Invariant {
            path: path.to_string(),
            message: "user-supplied descriptors may not declare [guard] or set \
                      run.supports_guard = true — the write guard installs native hook \
                      config and stays restricted to built-in descriptors until the guard \
                      engine is opened up (see the guard-engine follow-up issue). Remove \
                      it; unguarded runs fall back to the detect-stray-writes audit."
                .to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn embedded_sources_cover_every_bundled_descriptor() {
        let sources = embedded_sources();
        assert_eq!(sources.len(), EMBEDDED_DESCRIPTORS.len());
        assert!(sources.iter().all(|s| s.layer == Layer::Embedded));
        assert_eq!(sources[0].path, "harnesses/claude-code.toml");
        assert!(!sources[0].toml_src.is_empty());
    }

    #[test]
    fn user_layer_may_not_declare_a_guard_table() {
        let value: serde_json::Value = toml::from_str(
            "label = \"demo\"\n\n[guard]\nengine = \"claude-hooks\"\narmed_message = \"x\"\n",
        )
        .unwrap();
        let err = check_user_layer_restrictions(&value, "user.toml")
            .unwrap_err()
            .to_string();
        assert!(err.contains("user.toml"), "{err}");
        assert!(err.contains("may not declare [guard]"), "{err}");
        assert!(
            err.contains("detect-stray-writes"),
            "names the fallback: {err}"
        );
    }

    #[test]
    fn user_layer_may_not_flip_supports_guard_on() {
        let value: serde_json::Value =
            toml::from_str("label = \"demo\"\n\n[run]\nsupports_guard = true\n").unwrap();
        let err = check_user_layer_restrictions(&value, "user.toml")
            .unwrap_err()
            .to_string();
        assert!(err.contains("run.supports_guard"), "{err}");
    }

    #[test]
    fn user_layer_restrictions_pass_everything_else() {
        let value: serde_json::Value = toml::from_str(
            "label = \"demo\"\n\n[run]\nsupports_guard = false\n\n[model]\nflag = \"-m\"\n",
        )
        .unwrap();
        assert!(check_user_layer_restrictions(&value, "user.toml").is_ok());
    }

    #[test]
    fn config_root_prefers_explicit_env_then_xdg_then_home() {
        assert_eq!(
            config_root_from(Some("/explicit"), Some("/xdg"), Some(Path::new("/home/u"))),
            Some("/explicit".into())
        );
        assert_eq!(
            config_root_from(None, Some("/xdg"), Some(Path::new("/home/u"))),
            Some("/xdg/eval-magic".into())
        );
        assert_eq!(
            config_root_from(None, None, Some(Path::new("/home/u"))),
            Some("/home/u/.config/eval-magic".into())
        );
        assert_eq!(config_root_from(None, None, None), None);
    }

    #[test]
    fn empty_config_dir_env_disables_the_user_layer() {
        assert_eq!(
            config_root_from(Some(""), Some("/xdg"), Some(Path::new("/home/u"))),
            None
        );
    }

    #[test]
    fn discovery_layers_in_precedence_order() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config_root = tmp.path().join("config");
        let project_root = tmp.path().join("project");
        let user_dir = config_root.join("harnesses");
        let project_dir = project_root.join(".eval-magic").join("harnesses");
        fs::create_dir_all(&user_dir).unwrap();
        fs::create_dir_all(&project_dir).unwrap();
        // Written out of name order to prove the filename sort.
        fs::write(user_dir.join("b.toml"), "label = \"bee\"\n").unwrap();
        fs::write(user_dir.join("a.toml"), "label = \"ay\"\n").unwrap();
        fs::write(user_dir.join("notes.md"), "not a descriptor").unwrap();
        fs::write(project_dir.join("p.toml"), "label = \"pea\"\n").unwrap();
        let file = tmp.path().join("one-off.toml");
        fs::write(&file, "label = \"one-off\"\n").unwrap();

        let (sources, warnings) =
            discover_sources(Some(&config_root), &project_root, Some(&file)).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let tail: Vec<(Layer, &str)> = sources[EMBEDDED_DESCRIPTORS.len()..]
            .iter()
            .map(|s| (s.layer, s.path.as_str()))
            .collect();
        assert_eq!(sources[0].layer, Layer::Embedded);
        assert_eq!(
            tail,
            vec![
                (Layer::UserGlobal, user_dir.join("a.toml").to_str().unwrap()),
                (Layer::UserGlobal, user_dir.join("b.toml").to_str().unwrap()),
                (
                    Layer::ProjectLocal,
                    project_dir.join("p.toml").to_str().unwrap()
                ),
                (Layer::HarnessFile, file.to_str().unwrap()),
            ]
        );
    }

    #[test]
    fn discovery_tolerates_missing_layer_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (sources, warnings) = discover_sources(
            Some(&tmp.path().join("nope")),
            &tmp.path().join("also-nope"),
            None,
        )
        .unwrap();
        assert_eq!(sources.len(), EMBEDDED_DESCRIPTORS.len());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn discovery_fails_hard_on_an_unreadable_harness_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("missing.toml");
        let err = discover_sources(None, tmp.path(), Some(&missing))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--harness-file"), "{err}");
        assert!(err.contains("missing.toml"), "{err}");
    }
}
