//! Marker-driven profile detection over a task tree.
//!
//! Two packaged profile families ask the same question of a task environment —
//! "which ecosystems and tools does this codebase actually use?" — and answer it
//! from the same three signals: an exact filename, a single-`*` filename
//! pattern, and a `package.json` dependency. The guard's command policy
//! ([`crate::sandbox::guard_profiles`]) and the framework-ignore writer
//! ([`crate::workspace::tool_ignore`]) share this one walk so a marker added for
//! one is understood by the other.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

/// One packaged profile, as detection sees it.
pub trait TreeProfile {
    /// Stable identifier, e.g. `language/rust` or `tool/prettier`.
    fn id(&self) -> &str;
    /// Filenames that identify this profile exactly.
    fn markers(&self) -> &[String];
    /// Filename patterns with at most one `*`, e.g. `requirements*.txt`.
    fn marker_patterns(&self) -> &[String];
    /// `package.json` dependency names that identify this profile.
    fn package_json_dependencies(&self) -> &[String];
}

/// Every profile whose markers appear anywhere in `root`, sorted and deduped.
pub fn detect<'a>(
    root: &Path,
    profiles: impl IntoIterator<Item = &'a dyn TreeProfile>,
) -> io::Result<Vec<String>> {
    let profiles: Vec<&dyn TreeProfile> = profiles.into_iter().collect();
    let mut detected = BTreeSet::new();
    visit(root, &profiles, &mut detected)?;
    Ok(detected.into_iter().collect())
}

fn visit(
    path: &Path,
    profiles: &[&dyn TreeProfile],
    detected: &mut BTreeSet<String>,
) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            if !excluded_directory(&name) {
                visit(&entry.path(), profiles, detected)?;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        for profile in profiles {
            if profile.markers().iter().any(|marker| marker == &name)
                || profile
                    .marker_patterns()
                    .iter()
                    .any(|pattern| marker_matches(pattern, &name))
            {
                detected.insert(profile.id().to_string());
            }
        }
        if name == "package.json" {
            detect_package_json_profiles(&entry.path(), profiles, detected);
        }
    }
    Ok(())
}

fn marker_matches(pattern: &str, name: &str) -> bool {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return pattern == name;
    };
    !suffix.contains('*') && name.starts_with(prefix) && name.ends_with(suffix)
}

/// Directories detection never descends into: Git internals, framework-owned
/// state, and the build/dependency trees whose vendored copies of a marker say
/// nothing about the project itself.
fn excluded_directory(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".eval-magic-outputs"
            | ".claude"
            | ".codex"
            | ".agents"
            | ".opencode"
            | ".cline"
            | "target"
            | "node_modules"
            | ".venv"
    )
}

fn detect_package_json_profiles(
    path: &Path,
    profiles: &[&dyn TreeProfile],
    detected: &mut BTreeSet<String>,
) {
    let Ok(body) = fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
        return;
    };
    for profile in profiles {
        if profile
            .package_json_dependencies()
            .iter()
            .any(|dependency| {
                [
                    "dependencies",
                    "devDependencies",
                    "peerDependencies",
                    "optionalDependencies",
                ]
                .iter()
                .any(|field| {
                    value
                        .get(field)
                        .and_then(|deps| deps.get(dependency))
                        .is_some()
                })
            })
        {
            detected.insert(profile.id().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{TreeProfile, detect};

    struct Probe {
        id: &'static str,
        markers: Vec<String>,
        marker_patterns: Vec<String>,
        package_json_dependencies: Vec<String>,
    }

    impl Probe {
        fn new(id: &'static str) -> Self {
            Self {
                id,
                markers: Vec::new(),
                marker_patterns: Vec::new(),
                package_json_dependencies: Vec::new(),
            }
        }

        fn markers(mut self, values: &[&str]) -> Self {
            self.markers = values.iter().map(|value| value.to_string()).collect();
            self
        }

        fn marker_patterns(mut self, values: &[&str]) -> Self {
            self.marker_patterns = values.iter().map(|value| value.to_string()).collect();
            self
        }

        fn dependencies(mut self, values: &[&str]) -> Self {
            self.package_json_dependencies = values.iter().map(|value| value.to_string()).collect();
            self
        }
    }

    impl TreeProfile for Probe {
        fn id(&self) -> &str {
            self.id
        }
        fn markers(&self) -> &[String] {
            &self.markers
        }
        fn marker_patterns(&self) -> &[String] {
            &self.marker_patterns
        }
        fn package_json_dependencies(&self) -> &[String] {
            &self.package_json_dependencies
        }
    }

    #[test]
    fn detects_markers_patterns_and_dependencies_recursively_and_sorted() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("frontend")).unwrap();
        fs::write(
            root.path().join("frontend/package.json"),
            r#"{"devDependencies":{"prettier":"3.0.0"}}"#,
        )
        .unwrap();
        fs::create_dir_all(root.path().join("backend")).unwrap();
        fs::write(root.path().join("backend/requirements-dev.txt"), "").unwrap();
        fs::write(root.path().join("Cargo.toml"), "").unwrap();

        let profiles = [
            Probe::new("tool/prettier").dependencies(&["prettier"]),
            Probe::new("language/python").marker_patterns(&["requirements*.txt"]),
            Probe::new("language/rust").markers(&["Cargo.toml"]),
        ];
        let detected = detect(
            root.path(),
            profiles.iter().map(|probe| probe as &dyn TreeProfile),
        )
        .unwrap();

        assert_eq!(
            detected,
            ["language/python", "language/rust", "tool/prettier"]
        );
    }

    #[test]
    fn framework_and_build_directories_are_never_walked() {
        let root = tempdir().unwrap();
        for dir in [
            ".claude",
            ".git",
            "node_modules",
            "target",
            ".eval-magic-outputs",
        ] {
            fs::create_dir_all(root.path().join(dir)).unwrap();
            fs::write(root.path().join(dir).join("Cargo.toml"), "").unwrap();
        }

        let profiles = [Probe::new("language/rust").markers(&["Cargo.toml"])];
        let detected = detect(
            root.path(),
            profiles.iter().map(|probe| probe as &dyn TreeProfile),
        )
        .unwrap();

        assert!(
            detected.is_empty(),
            "walked an excluded directory: {detected:?}"
        );
    }
}
