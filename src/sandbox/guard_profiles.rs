//! Packaged command-policy profiles and task-tree auto-detection.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::core::GuardPolicyConfig;
use crate::core::tree_profiles::{TreeProfile, detect};

include!(concat!(env!("OUT_DIR"), "/guard_profiles.rs"));

#[derive(Debug, Deserialize)]
struct GuardProfile {
    id: String,
    #[serde(default)]
    markers: Vec<String>,
    #[serde(default)]
    marker_patterns: Vec<String>,
    #[serde(default)]
    package_json_dependencies: Vec<String>,
    #[serde(default)]
    allow_commands: Vec<String>,
}

impl TreeProfile for GuardProfile {
    fn id(&self) -> &str {
        &self.id
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

static PROFILES: LazyLock<HashMap<String, GuardProfile>> = LazyLock::new(|| {
    let mut profiles = HashMap::new();
    for (path, body) in PACKAGED_GUARD_PROFILES {
        let profile: GuardProfile = toml::from_str(body)
            .unwrap_or_else(|error| panic!("invalid guard profile {path}: {error}"));
        let id = profile.id.clone();
        assert!(
            profiles.insert(id.clone(), profile).is_none(),
            "duplicate guard profile {id}"
        );
    }
    profiles
});

pub(crate) fn has_profile(id: &str) -> bool {
    PROFILES.contains_key(id)
}

pub(crate) fn claims_command(actual: &[String]) -> bool {
    PROFILES.values().any(|profile| {
        profile.allow_commands.iter().any(|command| {
            let rule: Vec<&str> = command.split_whitespace().collect();
            actual.len() >= rule.len()
                && actual
                    .iter()
                    .zip(rule)
                    .all(|(word, expected)| word == expected)
        })
    })
}

/// Expand authored profile references into the exact policy frozen into run artifacts.
pub(crate) fn expand_policy(policy: &GuardPolicyConfig) -> Result<GuardPolicyConfig, String> {
    let mut expanded = policy.clone();
    for id in &policy.profiles {
        let profile = PROFILES
            .get(id)
            .ok_or_else(|| format!("unknown guard profile {id:?}"))?;
        expanded
            .allow_commands
            .extend(profile.allow_commands.clone());
    }
    expanded.allow_tools.sort();
    expanded.allow_tools.dedup();
    expanded.allow_commands.sort();
    expanded.allow_commands.dedup();
    Ok(expanded)
}

/// Detect every applicable packaged profile in a staged task tree.
pub(crate) fn detect_profiles(root: &Path) -> io::Result<Vec<String>> {
    detect(
        root,
        PROFILES.values().map(|profile| profile as &dyn TreeProfile),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{PROFILES, detect_profiles, expand_policy};
    use crate::core::GuardPolicyConfig;
    use crate::sandbox::command_policy::validate_policy_syntax;

    #[test]
    fn packaged_profiles_contain_valid_command_rules() {
        for profile in PROFILES.values() {
            let policy = GuardPolicyConfig {
                allow_commands: profile.allow_commands.clone(),
                ..GuardPolicyConfig::default()
            };
            validate_policy_syntax(&policy)
                .unwrap_or_else(|error| panic!("profile {}: {error}", profile.id));
        }
    }

    #[test]
    fn explicit_profiles_expand_without_adding_detected_profiles() {
        let policy = GuardPolicyConfig {
            profiles: vec!["language/rust".to_string()],
            ..GuardPolicyConfig::default()
        };

        let expanded = expand_policy(&policy).unwrap();

        assert!(expanded.allow_commands.contains(&"cargo test".to_string()));
        assert!(!expanded.allow_commands.contains(&"npm test".to_string()));
    }

    /// Issue #297: a plain package.json project (the pinned Weeknight fixture
    /// is Vite + React) must be able to start its own dev server. `dev` and
    /// `start` are generic lifecycle script names, not Next.js-specific, so
    /// they belong to the language profile.
    #[test]
    fn language_javascript_allows_dev_and_start_scripts() {
        let policy = GuardPolicyConfig {
            profiles: vec!["language/javascript".to_string()],
            ..GuardPolicyConfig::default()
        };

        let expanded = expand_policy(&policy).unwrap();

        for command in [
            "npm run dev",
            "npm run start",
            "pnpm run dev",
            "pnpm run start",
            "yarn run dev",
            "yarn run start",
            "bun run dev",
            "bun run start",
        ] {
            assert!(
                expanded.allow_commands.contains(&command.to_string()),
                "{command}"
            );
        }
    }

    #[test]
    fn detection_joins_language_and_framework_profiles_recursively() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("frontend")).unwrap();
        fs::write(
            root.path().join("frontend/package.json"),
            r#"{"dependencies":{"next":"15.0.0"}}"#,
        )
        .unwrap();
        fs::create_dir_all(root.path().join("backend")).unwrap();
        fs::write(root.path().join("backend/requirements-dev.txt"), "").unwrap();

        let detected = detect_profiles(root.path()).unwrap();

        assert_eq!(
            detected,
            ["framework/nextjs", "language/javascript", "language/python"]
        );
    }
}
