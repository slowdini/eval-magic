//! Live-skill shadow detector and reporting for Codex.
//!
//! Codex discovers skills from repository-ancestor `.agents/skills`
//! directories, the user's `~/.agents/skills`, `/etc/codex/skills`, and
//! enabled installed plugins. A logical eval skill present in any of those
//! sources contaminates the control arm even when eval-magic stages its test
//! copy under a generated name. Detection is deliberately best-effort:
//! unreadable/malformed skills and an unavailable or invalid plugin listing
//! are ignored, while all other sources continue to be scanned.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::adapters::skill_shadow::{
    PluginShadowReport, ShadowNamespace, ShadowRelation, ShadowRoot, ShadowRootScope, ShadowSource,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginList {
    #[serde(default)]
    installed: Vec<InstalledPlugin>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstalledPlugin {
    plugin_id: Option<String>,
    name: String,
    marketplace_name: String,
    version: String,
    #[serde(default)]
    installed: bool,
    #[serde(default)]
    enabled: bool,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn user_home() -> PathBuf {
    env_path("HOME").unwrap_or_else(|| std::env::home_dir().unwrap_or_default())
}

fn codex_home(home: &Path) -> PathBuf {
    env_path("CODEX_HOME").unwrap_or_else(|| home.join(".codex"))
}

fn plugin_list_json() -> Option<String> {
    let output = Command::new("codex")
        .args(["plugin", "list", "--json"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].trim()
    } else {
        value
    }
}

/// Read the top-level `name:` from a skill's YAML frontmatter. Codex keys
/// discovery by this value, not necessarily by the enclosing folder name.
fn frontmatter_name(skill_md: &Path) -> Option<String> {
    let raw = fs::read_to_string(skill_md).ok()?;
    let mut lines = raw.lines();
    (lines.next()?.trim() == "---").then_some(())?;
    let mut found = None;
    for line in lines {
        if line.trim() == "---" {
            return found;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() == "name" {
            let name = unquote(value);
            found = (!name.is_empty()).then(|| name.to_string());
        }
    }
    None
}

fn direct_skill_sources(
    dir: &Path,
    scope: ShadowRootScope,
    namespace: ShadowNamespace,
) -> Vec<ShadowSource> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let skill_name = frontmatter_name(&path.join("SKILL.md"))?;
            Some(ShadowSource::live_skill(
                skill_name,
                &path,
                ShadowRoot {
                    scope,
                    namespace,
                    plugin: None,
                    path: dir.to_string_lossy().into_owned(),
                    relation: ShadowRelation::Native,
                },
                format!(
                    "Move or rename the conflicting skill directory '{}' before dispatch.",
                    path.display()
                ),
            ))
        })
        .collect()
}

fn repository_skill_dirs(scan_root: &Path) -> Vec<PathBuf> {
    let Some(repo_root) = scan_root
        .ancestors()
        .find(|path| path.join(".git").exists())
    else {
        return Vec::new();
    };
    if repo_root == scan_root {
        return Vec::new();
    }
    let mut dirs = Vec::new();
    let mut cursor = scan_root.parent();
    while let Some(path) = cursor {
        dirs.push(path.join(".agents/skills"));
        if path == repo_root {
            break;
        }
        cursor = path.parent();
    }
    dirs
}

fn plugin_skill_sources(codex_home: &Path, raw: Option<&str>) -> Vec<ShadowSource> {
    let Some(list) = raw.and_then(|json| serde_json::from_str::<PluginList>(json).ok()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for plugin in list
        .installed
        .into_iter()
        .filter(|plugin| plugin.installed && plugin.enabled)
    {
        let label = plugin
            .plugin_id
            .unwrap_or_else(|| format!("{}@{}", plugin.name, plugin.marketplace_name));
        let skills_dir = codex_home
            .join("plugins/cache")
            .join(&plugin.marketplace_name)
            .join(&plugin.name)
            .join(&plugin.version)
            .join("skills");
        let Ok(entries) = fs::read_dir(&skills_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(skill_name) = frontmatter_name(&path.join("SKILL.md")) else {
                continue;
            };
            out.push(ShadowSource::live_plugin(
                label.clone(),
                skill_name.clone(),
                skill_name,
                &path,
                ShadowRoot {
                    scope: ShadowRootScope::Global,
                    namespace: ShadowNamespace::Plugin,
                    plugin: Some(label.clone()),
                    path: skills_dir.to_string_lossy().into_owned(),
                    relation: ShadowRelation::Native,
                },
                format!(
                    "Add '--disable plugins' to every Codex dispatch to disable plugin '{label}'."
                ),
            ));
        }
    }
    out
}

fn sort_and_dedup(sources: &mut Vec<ShadowSource>) {
    sources.sort_by(|a, b| {
        (&a.plugin, &a.skill_name, &a.discovery_path).cmp(&(
            &b.plugin,
            &b.skill_name,
            &b.discovery_path,
        ))
    });
    sources.dedup();
}

fn detect_with_sources(
    scan_root: &Path,
    staged_skill_names: &[&str],
    home: &Path,
    codex_home: &Path,
    admin_skills: &Path,
    plugin_json: Option<&str>,
) -> PluginShadowReport {
    let staged: HashSet<&str> = staged_skill_names.iter().copied().collect();
    let mut seen_dirs = HashSet::new();
    let mut shadowed = Vec::new();
    for dir in repository_skill_dirs(scan_root) {
        if seen_dirs.insert(dir.clone()) {
            shadowed.extend(direct_skill_sources(
                &dir,
                ShadowRootScope::Project,
                ShadowNamespace::Agents,
            ));
        }
    }
    let user_skills = home.join(".agents/skills");
    if seen_dirs.insert(user_skills.clone()) {
        shadowed.extend(direct_skill_sources(
            &user_skills,
            ShadowRootScope::Global,
            ShadowNamespace::Agents,
        ));
    }
    if seen_dirs.insert(admin_skills.to_path_buf()) {
        shadowed.extend(direct_skill_sources(
            admin_skills,
            ShadowRootScope::Admin,
            ShadowNamespace::Codex,
        ));
    }
    shadowed.extend(plugin_skill_sources(codex_home, plugin_json));
    shadowed.retain(|source| staged.contains(source.skill_name()));
    sort_and_dedup(&mut shadowed);

    PluginShadowReport::from_sources(codex_home.to_string_lossy(), shadowed)
}

/// Detect logical eval skill names that Codex can also load from live sources.
pub fn shadow_preflight(
    scan_root: &Path,
    staged_skill_names: &[&str],
) -> Option<PluginShadowReport> {
    let home = user_home();
    let codex_home = codex_home(&home);
    let plugin_json = plugin_list_json();
    let report = detect_with_sources(
        scan_root,
        staged_skill_names,
        &home,
        &codex_home,
        Path::new("/etc/codex/skills"),
        plugin_json.as_deref(),
    );
    (!report.is_empty()).then_some(report)
}

/// Compatibility entry point; v2 warnings are rendered by the shared policy.
pub fn shadow_validity_warnings(report: &PluginShadowReport) -> Vec<String> {
    crate::adapters::skill_shadow::shadow_validity_warnings(report)
}

/// Compatibility entry point; v2 banners are rendered by the shared policy.
pub fn format_shadow_banner(report: &PluginShadowReport) -> String {
    crate::adapters::skill_shadow::format_shadow_banner(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::skill_shadow::ShadowSourceKind;
    use serde_json::json;
    use tempfile::TempDir;

    fn write_skill(path: &Path, name: &str) {
        fs::create_dir_all(path).unwrap();
        fs::write(
            path.join("SKILL.md"),
            format!("---\nname: '{name}'\ndescription: test\n---\n"),
        )
        .unwrap();
    }

    #[test]
    fn direct_scan_uses_frontmatter_name_and_skips_staged_env() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        let scan_root = repo.join(".eval-magic/skill/iteration-1/env-g1-with_skill");
        fs::create_dir_all(repo.join(".git")).unwrap();
        write_skill(
            &repo.join(".agents/skills/different-folder"),
            "target-skill",
        );
        write_skill(
            &scan_root.join(".agents/skills/staged-copy"),
            "target-skill",
        );

        let report = detect_with_sources(
            &scan_root,
            &["target-skill"],
            &tmp.path().join("home"),
            &tmp.path().join("codex-home"),
            &tmp.path().join("etc-skills"),
            None,
        );

        assert_eq!(report.source_count(), 1);
        assert_eq!(report.source(0).kind, ShadowSourceKind::Skill);
        assert!(
            report
                .source(0)
                .discovery_path
                .ends_with("different-folder")
        );
    }

    #[test]
    fn enabled_plugin_scan_uses_installed_cache_layout() {
        let tmp = TempDir::new().unwrap();
        let codex_home = tmp.path().join("codex-home");
        let skill = codex_home.join("plugins/cache/slowdini/slow-powers/0.5.3/skills/review");
        write_skill(&skill, "mr-review");
        let plugin_json = json!({
            "installed": [
                {
                    "pluginId": "slow-powers@slowdini",
                    "name": "slow-powers",
                    "marketplaceName": "slowdini",
                    "version": "0.5.3",
                    "installed": true,
                    "enabled": true
                },
                {
                    "pluginId": "disabled@slowdini",
                    "name": "slow-powers",
                    "marketplaceName": "slowdini",
                    "version": "0.5.3",
                    "installed": true,
                    "enabled": false
                }
            ]
        })
        .to_string();

        let report = detect_with_sources(
            tmp.path(),
            &["mr-review"],
            &tmp.path().join("home"),
            &codex_home,
            &tmp.path().join("etc-skills"),
            Some(&plugin_json),
        );

        assert_eq!(report.source_count(), 1);
        let source = report.source(0);
        assert_eq!(source.kind, ShadowSourceKind::Plugin);
        assert_eq!(source.plugin.as_deref(), Some("slow-powers@slowdini"));
        assert_eq!(source.skill_name, "mr-review");
    }

    #[test]
    fn invalid_plugin_list_does_not_hide_direct_skills() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write_skill(&home.join(".agents/skills/review"), "mr-review");

        let report = detect_with_sources(
            tmp.path(),
            &["mr-review"],
            &home,
            &tmp.path().join("codex-home"),
            &tmp.path().join("etc-skills"),
            Some("not json"),
        );

        assert_eq!(report.source_count(), 1);
        assert_eq!(report.source(0).kind, ShadowSourceKind::Skill);
    }

    #[test]
    fn malformed_skills_do_not_create_false_reports() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let malformed = home.join(".agents/skills/review");
        fs::create_dir_all(&malformed).unwrap();
        fs::write(malformed.join("SKILL.md"), "name: mr-review\n").unwrap();
        let unclosed = home.join(".agents/skills/unclosed");
        fs::create_dir_all(&unclosed).unwrap();
        fs::write(unclosed.join("SKILL.md"), "---\nname: mr-review\n").unwrap();

        let report = detect_with_sources(
            tmp.path(),
            &["mr-review"],
            &home,
            &tmp.path().join("codex-home"),
            &tmp.path().join("etc-skills"),
            None,
        );

        assert!(report.is_empty());
    }

    #[test]
    fn plugin_shadow_guidance_names_runtime_disable_and_conservative_warning() {
        let skill_path = Path::new("/codex/plugins/cache/slowdini/slow-powers/1/skills/mr-review");
        let report = PluginShadowReport::from_sources(
            "/codex",
            vec![ShadowSource::live_plugin(
                "slow-powers@slowdini",
                "mr-review",
                "mr-review",
                skill_path,
                ShadowRoot {
                    scope: ShadowRootScope::Global,
                    namespace: ShadowNamespace::Plugin,
                    plugin: Some("slow-powers@slowdini".into()),
                    path: skill_path.parent().unwrap().display().to_string(),
                    relation: ShadowRelation::Native,
                },
                "Add '--disable plugins' to every Codex dispatch.",
            )],
        );

        let banner = format_shadow_banner(&report);
        assert!(banner.contains("--disable plugins"), "{banner}");
        assert!(banner.contains("slow-powers@slowdini"), "{banner}");
        assert!(banner.contains("remediation"), "{banner}");

        let warning = shadow_validity_warnings(&report).join("\n");
        assert!(warning.contains("--disable plugins"), "{warning}");
        assert!(warning.contains("comparison invalid"), "{warning}");
    }

    #[test]
    fn direct_skill_shadow_guidance_does_not_recommend_plugin_disable() {
        let skill_path = Path::new("/repo/.agents/skills/mr-review");
        let report = PluginShadowReport::from_sources(
            "/codex",
            vec![ShadowSource::live_skill(
                "mr-review",
                skill_path,
                ShadowRoot {
                    scope: ShadowRootScope::Project,
                    namespace: ShadowNamespace::Agents,
                    plugin: None,
                    path: "/repo/.agents/skills".into(),
                    relation: ShadowRelation::Native,
                },
                "Move or rename the conflicting skill directory.",
            )],
        );

        let warning = shadow_validity_warnings(&report).join("\n");
        assert!(!warning.contains("--disable plugins"), "{warning}");
        assert!(
            warning.to_lowercase().contains("move or rename"),
            "{warning}"
        );
    }
}
