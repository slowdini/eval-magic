//! Plugin-shadow detector (Claude Code).
//!
//! The runner stages eval skills into each dispatch's project-local
//! `.claude/skills/` dir, but every `claude -p` dispatch ALSO loads the user/global
//! plugins and the global skills dir from its Claude config. When a staged skill
//! name collides with one of those, both copies are discoverable and the
//! with/without comparison is contaminated. The runner cannot strip an installed
//! plugin from a dispatch, so this module only *detects and reports* the overlap,
//! reading declared settings as a best-effort proxy.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const ISOLATION_DOC: &str = "README.md → Claude Code → \"Isolating from installed plugins\"";

/// A staged skill that is also discoverable from the live environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ShadowSource {
    Plugin {
        plugin: String,
        skill_name: String,
        path: String,
    },
    GlobalSkill {
        skill_name: String,
        path: String,
    },
}

impl ShadowSource {
    fn skill_name(&self) -> &str {
        match self {
            ShadowSource::Plugin { skill_name, .. } => skill_name,
            ShadowSource::GlobalSkill { skill_name, .. } => skill_name,
        }
    }

    fn source_label(&self) -> String {
        match self {
            ShadowSource::Plugin { plugin, .. } => format!("enabled plugin '{plugin}'"),
            ShadowSource::GlobalSkill { .. } => "the global skills dir".to_string(),
        }
    }
}

/// The detector's findings for a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginShadowReport {
    pub config_dir: String,
    pub shadowed: Vec<ShadowSource>,
}

/// The Claude Code config dir: a non-empty `CLAUDE_CONFIG_DIR` override (passed
/// in), else `~/.claude`.
pub fn resolve_config_dir(config_dir_override: Option<&str>) -> PathBuf {
    match config_dir_override {
        Some(o) if !o.trim().is_empty() => PathBuf::from(o),
        _ => std::env::home_dir().unwrap_or_default().join(".claude"),
    }
}

/// The Claude Code config dir, reading the `CLAUDE_CONFIG_DIR` override from the
/// environment (else `~/.claude`). Thin convenience over [`resolve_config_dir`]
/// for the call sites that should honor the env var — the override logic itself
/// is covered by `resolve_config_dir`'s tests.
pub fn config_dir_from_env() -> PathBuf {
    resolve_config_dir(std::env::var("CLAUDE_CONFIG_DIR").ok().as_deref())
}

fn read_json_safe<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

#[derive(Debug, Deserialize)]
struct Settings {
    #[serde(rename = "enabledPlugins")]
    enabled_plugins: Option<HashMap<String, bool>>,
}

/// Effective `enabledPlugins` map, honoring Claude Code's settings precedence
/// (local > project > user). Later sources override earlier keys, so a
/// project-scope `false` correctly masks a user-scope `true`.
fn resolve_enabled_plugins(config_dir: &Path, cwd: &Path) -> HashMap<String, bool> {
    let sources = [
        config_dir.join("settings.json"),
        cwd.join(".claude").join("settings.json"),
        cwd.join(".claude").join("settings.local.json"),
    ];
    let mut merged = HashMap::new();
    for path in sources {
        if let Some(s) = read_json_safe::<Settings>(&path)
            && let Some(ep) = s.enabled_plugins
        {
            merged.extend(ep);
        }
    }
    merged
}

/// Names of skill folders (those holding a `SKILL.md`) directly under `dir`.
fn skill_folder_names(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("SKILL.md").exists() {
            out.push((entry.file_name().to_string_lossy().into_owned(), path));
        }
    }
    out
}

#[derive(Debug, Deserialize)]
struct Install {
    #[serde(rename = "installPath")]
    install_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InstalledPlugins {
    plugins: Option<HashMap<String, Vec<Install>>>,
}

/// Skills exposed by currently-enabled installed plugins.
fn enabled_plugin_skills(config_dir: &Path, enabled: &HashMap<String, bool>) -> Vec<ShadowSource> {
    let mut out = Vec::new();
    let manifest: Option<InstalledPlugins> =
        read_json_safe(&config_dir.join("plugins").join("installed_plugins.json"));
    let Some(plugins) = manifest.and_then(|m| m.plugins) else {
        return out;
    };
    for (key, installs) in plugins {
        if enabled.get(&key) != Some(&true) {
            continue; // only enabled plugins shadow
        }
        for inst in installs {
            let Some(install_path) = inst.install_path else {
                continue;
            };
            for (name, path) in skill_folder_names(&Path::new(&install_path).join("skills")) {
                out.push(ShadowSource::Plugin {
                    plugin: key.clone(),
                    skill_name: name,
                    path: path.to_string_lossy().into_owned(),
                });
            }
        }
    }
    out
}

/// Skills under the global skills dir (`<config_dir>/skills`).
fn global_skills(config_dir: &Path) -> Vec<ShadowSource> {
    skill_folder_names(&config_dir.join("skills"))
        .into_iter()
        .map(|(name, path)| ShadowSource::GlobalSkill {
            skill_name: name,
            path: path.to_string_lossy().into_owned(),
        })
        .collect()
}

/// Which of `staged_skill_names` are also discoverable from enabled plugins or
/// the global skills dir. Matches on the skill folder name (exact).
pub fn detect_plugin_shadows(
    config_dir: &Path,
    cwd: &Path,
    staged_skill_names: &[&str],
) -> PluginShadowReport {
    let staged: HashSet<&str> = staged_skill_names.iter().copied().collect();
    let enabled = resolve_enabled_plugins(config_dir, cwd);
    let mut shadowed = Vec::new();

    for s in enabled_plugin_skills(config_dir, &enabled) {
        if staged.contains(s.skill_name()) {
            shadowed.push(s);
        }
    }
    for s in global_skills(config_dir) {
        if staged.contains(s.skill_name()) {
            shadowed.push(s);
        }
    }

    PluginShadowReport {
        config_dir: config_dir.to_string_lossy().into_owned(),
        shadowed,
    }
}

/// One `validity_warnings` line per shadowed skill (for benchmark.json).
pub fn shadow_validity_warnings(report: &PluginShadowReport) -> Vec<String> {
    report
        .shadowed
        .iter()
        .map(|s| {
            format!(
                "staged skill '{}' is also provided by {} — each claude -p dispatch could discover \
                 both copies, so with/without results may be contaminated. Isolate each dispatch's \
                 Claude config (see {}).",
                s.skill_name(),
                s.source_label(),
                ISOLATION_DOC
            )
        })
        .collect()
}

/// Build-time banner for the runner. Empty string when nothing is shadowed.
pub fn format_shadow_banner(report: &PluginShadowReport) -> String {
    if report.shadowed.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        String::new(),
        "⚠ Plugin-shadow warning: skills staged for this eval are ALSO discoverable".to_string(),
        "  from your live environment:".to_string(),
    ];
    for s in &report.shadowed {
        lines.push(format!("  • {} — {}", s.skill_name(), s.source_label()));
    }
    lines.push(
        "  Each `claude -p` dispatch loads your user/global plugins and skills, so".to_string(),
    );
    lines.push("  both the staged copy and the installed copy are discoverable — the".to_string());
    lines.push(
        "  with/without comparison may be contaminated and the control arm is not truly"
            .to_string(),
    );
    lines.push(
        "  skill-absent. The runner cannot strip an installed plugin from the dispatch."
            .to_string(),
    );
    lines.push(format!(
        "  Isolate each dispatch's Claude config — see {ISOLATION_DOC}."
    ));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn fresh(dir: &TempDir) -> (PathBuf, PathBuf) {
        let config = dir.path().join("config");
        let cwd = dir.path().join("cwd");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        (config, cwd)
    }

    fn write_file(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn install_plugin(config: &Path, key: &str, skill_names: &[&str]) -> PathBuf {
        let install = config
            .join("plugins")
            .join("cache")
            .join(key.replace('@', "__"));
        for name in skill_names {
            write_file(
                &install.join("skills").join(name).join("SKILL.md"),
                &format!("---\nname: {name}\ndescription: x\n---\n"),
            );
        }
        install
    }

    fn write_installed_manifest(config: &Path, entries: &[(&str, &Path)]) {
        let mut plugins = serde_json::Map::new();
        for (key, install) in entries {
            plugins.insert(
                (*key).to_string(),
                json!([{"installPath": install.to_string_lossy()}]),
            );
        }
        write_file(
            &config.join("plugins").join("installed_plugins.json"),
            &serde_json::to_string_pretty(&json!({"version": 2, "plugins": plugins})).unwrap(),
        );
    }

    fn write_settings(path: &Path, enabled: &[(&str, bool)]) {
        let mut m = serde_json::Map::new();
        for (k, v) in enabled {
            m.insert((*k).to_string(), json!(v));
        }
        write_file(
            path,
            &serde_json::to_string(&json!({"enabledPlugins": m})).unwrap(),
        );
    }

    #[test]
    fn honors_config_dir_override() {
        assert_eq!(
            resolve_config_dir(Some("/custom/cfg")),
            PathBuf::from("/custom/cfg")
        );
    }

    #[test]
    fn defaults_to_home_claude_when_unset() {
        let expected = std::env::home_dir().unwrap_or_default().join(".claude");
        assert_eq!(resolve_config_dir(None), expected);
        // Whitespace-only override falls back to the default too.
        assert_eq!(resolve_config_dir(Some("  ")), expected);
    }

    #[test]
    fn flags_skill_also_provided_by_enabled_plugin() {
        let tmp = TempDir::new().unwrap();
        let (config, cwd) = fresh(&tmp);
        let ip = install_plugin(
            &config,
            "slow-powers@slowdini",
            &["verification-before-completion", "writing-skills"],
        );
        write_installed_manifest(&config, &[("slow-powers@slowdini", &ip)]);
        write_settings(
            &config.join("settings.json"),
            &[("slow-powers@slowdini", true)],
        );

        let report = detect_plugin_shadows(&config, &cwd, &["verification-before-completion"]);
        assert_eq!(report.shadowed.len(), 1);
        match &report.shadowed[0] {
            ShadowSource::Plugin {
                plugin, skill_name, ..
            } => {
                assert_eq!(plugin, "slow-powers@slowdini");
                assert_eq!(skill_name, "verification-before-completion");
            }
            other => panic!("expected plugin shadow, got {other:?}"),
        }
    }

    #[test]
    fn does_not_flag_disabled_plugin() {
        let tmp = TempDir::new().unwrap();
        let (config, cwd) = fresh(&tmp);
        let ip = install_plugin(
            &config,
            "slow-powers@slowdini",
            &["verification-before-completion"],
        );
        write_installed_manifest(&config, &[("slow-powers@slowdini", &ip)]);
        write_settings(
            &config.join("settings.json"),
            &[("slow-powers@slowdini", false)],
        );

        let report = detect_plugin_shadows(&config, &cwd, &["verification-before-completion"]);
        assert_eq!(report.shadowed.len(), 0);
    }

    #[test]
    fn project_settings_disabling_user_enabled_plugin_suppresses_shadow() {
        let tmp = TempDir::new().unwrap();
        let (config, cwd) = fresh(&tmp);
        let ip = install_plugin(
            &config,
            "slow-powers@slowdini",
            &["verification-before-completion"],
        );
        write_installed_manifest(&config, &[("slow-powers@slowdini", &ip)]);
        write_settings(
            &config.join("settings.json"),
            &[("slow-powers@slowdini", true)],
        );
        // Project scope (cwd/.claude/settings.json) outranks user scope.
        write_settings(
            &cwd.join(".claude").join("settings.json"),
            &[("slow-powers@slowdini", false)],
        );

        let report = detect_plugin_shadows(&config, &cwd, &["verification-before-completion"]);
        assert_eq!(report.shadowed.len(), 0);
    }

    #[test]
    fn flags_skill_also_in_global_skills_dir() {
        let tmp = TempDir::new().unwrap();
        let (config, cwd) = fresh(&tmp);
        write_file(
            &config.join("skills").join("my-skill").join("SKILL.md"),
            "---\nname: my-skill\n---\n",
        );

        let report = detect_plugin_shadows(&config, &cwd, &["my-skill"]);
        assert_eq!(report.shadowed.len(), 1);
        match &report.shadowed[0] {
            ShadowSource::GlobalSkill { skill_name, .. } => assert_eq!(skill_name, "my-skill"),
            other => panic!("expected global-skill shadow, got {other:?}"),
        }
    }

    #[test]
    fn no_shadow_when_staged_names_match_nothing() {
        let tmp = TempDir::new().unwrap();
        let (config, cwd) = fresh(&tmp);
        let ip = install_plugin(&config, "p@m", &["other"]);
        write_installed_manifest(&config, &[("p@m", &ip)]);
        write_settings(&config.join("settings.json"), &[("p@m", true)]);

        let report = detect_plugin_shadows(&config, &cwd, &["mine"]);
        assert_eq!(report.shadowed.len(), 0);
    }

    #[test]
    fn graceful_when_config_dir_has_no_plugins_or_skills() {
        let tmp = TempDir::new().unwrap();
        let (config, cwd) = fresh(&tmp);
        let report = detect_plugin_shadows(&config, &cwd, &["x"]);
        assert_eq!(report.shadowed.len(), 0);
        assert_eq!(report.config_dir, config.to_string_lossy());
    }

    fn sample_report() -> PluginShadowReport {
        PluginShadowReport {
            config_dir: "/x".into(),
            shadowed: vec![ShadowSource::Plugin {
                plugin: "slow-powers@slowdini".into(),
                skill_name: "verification-before-completion".into(),
                path: "/p".into(),
            }],
        }
    }

    #[test]
    fn validity_warnings_name_skill_plugin_and_contamination() {
        let warnings = shadow_validity_warnings(&sample_report());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("verification-before-completion"));
        assert!(warnings[0].contains("slow-powers@slowdini"));
        assert!(warnings[0].to_lowercase().contains("contaminat"));
    }

    #[test]
    fn banner_is_empty_when_nothing_shadowed() {
        let empty = PluginShadowReport {
            config_dir: "/x".into(),
            shadowed: vec![],
        };
        assert_eq!(format_shadow_banner(&empty), "");
    }

    #[test]
    fn banner_lists_shadowed_skills_and_points_at_isolation_docs() {
        let banner = format_shadow_banner(&sample_report());
        assert!(banner.contains("verification-before-completion"));
        assert!(banner.contains("slow-powers@slowdini"));
        assert!(banner.to_lowercase().contains("isolat"));
    }
}
