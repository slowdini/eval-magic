//! Live-skill shadow detector and reporting for OpenCode.
//!
//! OpenCode discovers skills from more roots than any other built-in harness:
//! project `.opencode/skills`, `.claude/skills`, and `.agents/skills` (walked
//! up from the dispatch cwd to the git worktree), and global
//! `$XDG_CONFIG_HOME/opencode/skills` (default `~/.config/opencode/skills`),
//! `$OPENCODE_CONFIG_DIR/skills` (additive, not a replacement), the legacy
//! `~/.opencode/skills`, `~/.claude/skills`, and `~/.agents/skills`. The
//! `.claude`/`.agents` roots are a cross-harness contamination vector: a skill
//! installed for Claude Code or Codex is visible to OpenCode sessions by
//! default. A logical eval skill present in any of these sources contaminates
//! the control arm even though eval-magic stages its test copy under a
//! generated slug. Detection is deliberately best-effort: missing directories
//! and unreadable/malformed skills are ignored while all other roots continue
//! to be scanned.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::adapters::skill_shadow::{
    PluginShadowReport, ShadowNamespace, ShadowRelation, ShadowRoot, ShadowRootScope, ShadowSource,
};

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn user_home() -> PathBuf {
    env_path("HOME").unwrap_or_else(|| std::env::home_dir().unwrap_or_default())
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

/// Read the top-level `name:` from a skill's YAML frontmatter. OpenCode keys
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
            let relation = if namespace == ShadowNamespace::Opencode {
                ShadowRelation::Native
            } else {
                ShadowRelation::CrossHarness
            };
            let remediation = match namespace {
                ShadowNamespace::Claude => format!(
                    "Set OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1 for every dispatch, or move '{}'.",
                    path.display()
                ),
                ShadowNamespace::Agents => format!(
                    "Set OPENCODE_DISABLE_EXTERNAL_SKILLS=1 for every dispatch, or move '{}'.",
                    path.display()
                ),
                _ => format!(
                    "Move or rename the conflicting skill directory '{}'.",
                    path.display()
                ),
            };
            Some(ShadowSource::live_skill(
                skill_name,
                &path,
                ShadowRoot {
                    scope,
                    namespace,
                    plugin: None,
                    path: dir.to_string_lossy().into_owned(),
                    relation,
                },
                remediation,
            ))
        })
        .collect()
}

/// The three project skill dirs OpenCode checks at each level of the
/// up-from-cwd walk.
const PROJECT_SKILL_DIRS: [(&str, ShadowNamespace); 3] = [
    (".opencode/skills", ShadowNamespace::Opencode),
    (".claude/skills", ShadowNamespace::Claude),
    (".agents/skills", ShadowNamespace::Agents),
];

fn repository_skill_dirs(scan_root: &Path) -> Vec<(PathBuf, ShadowNamespace, ShadowRootScope)> {
    let Some(repo_root) = scan_root
        .ancestors()
        .find(|path| path.join(".git").exists())
    else {
        return Vec::new();
    };
    // A task-local repository makes the staged env itself the worktree root.
    // Its intentional staged skills are already skipped, and OpenCode's
    // project walk must not continue into the parent eval workspace.
    if repo_root == scan_root {
        return Vec::new();
    }
    let mut dirs = Vec::new();
    // Start at the parent: scan_root is the staged env itself, whose own
    // skills dir holds the intentional staged copies.
    let mut cursor = scan_root.parent();
    while let Some(path) = cursor {
        for (sub, namespace) in PROJECT_SKILL_DIRS {
            dirs.push((path.join(sub), namespace, ShadowRootScope::Project));
        }
        if path == repo_root {
            break;
        }
        cursor = path.parent();
    }
    dirs
}

/// The global skill roots: the xdg default config dir, `$OPENCODE_CONFIG_DIR`
/// (additive), the legacy `~/.opencode`, and the two cross-harness dirs.
fn global_skill_dirs(
    home: &Path,
    xdg_config_home: Option<&Path>,
    opencode_config_dir: Option<&Path>,
) -> Vec<(PathBuf, ShadowNamespace, ShadowRootScope)> {
    let mut dirs = vec![(
        default_config_dir(home, xdg_config_home).join("skills"),
        ShadowNamespace::Opencode,
        ShadowRootScope::Global,
    )];
    if let Some(dir) = opencode_config_dir {
        dirs.push((
            dir.join("skills"),
            ShadowNamespace::Opencode,
            ShadowRootScope::Global,
        ));
    }
    dirs.push((
        home.join(".opencode/skills"),
        ShadowNamespace::Opencode,
        ShadowRootScope::Global,
    ));
    dirs.push((
        home.join(".claude/skills"),
        ShadowNamespace::Claude,
        ShadowRootScope::Global,
    ));
    dirs.push((
        home.join(".agents/skills"),
        ShadowNamespace::Agents,
        ShadowRootScope::Global,
    ));
    dirs
}

fn default_config_dir(home: &Path, xdg_config_home: Option<&Path>) -> PathBuf {
    xdg_config_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".config"))
        .join("opencode")
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
    xdg_config_home: Option<&Path>,
    opencode_config_dir: Option<&Path>,
) -> PluginShadowReport {
    let staged: HashSet<&str> = staged_skill_names.iter().copied().collect();
    let mut dirs = repository_skill_dirs(scan_root);
    dirs.extend(global_skill_dirs(
        home,
        xdg_config_home,
        opencode_config_dir,
    ));

    let mut seen_dirs = HashSet::new();
    let mut shadowed = Vec::new();
    for (dir, namespace, scope) in dirs {
        if seen_dirs.insert(dir.clone()) {
            shadowed.extend(direct_skill_sources(&dir, scope, namespace));
        }
    }
    shadowed.retain(|source| staged.contains(source.skill_name()));
    sort_and_dedup(&mut shadowed);

    PluginShadowReport::from_sources(
        default_config_dir(home, xdg_config_home).to_string_lossy(),
        shadowed,
    )
}

/// Detect logical eval skill names that OpenCode can also load from live
/// sources.
pub fn shadow_preflight(
    scan_root: &Path,
    staged_skill_names: &[&str],
) -> Option<PluginShadowReport> {
    let home = user_home();
    let report = detect_with_sources(
        scan_root,
        staged_skill_names,
        &home,
        env_path("XDG_CONFIG_HOME").as_deref(),
        env_path("OPENCODE_CONFIG_DIR").as_deref(),
    );
    (!report.is_empty()).then_some(report)
}

fn collect_debug_skill_locations(
    value: &serde_json::Value,
    inherited_name: Option<&str>,
    locations: &mut BTreeMap<String, String>,
) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                collect_debug_skill_locations(value, None, locations);
            }
        }
        serde_json::Value::Object(object) => {
            let name = object
                .get("name")
                .and_then(serde_json::Value::as_str)
                .or(inherited_name);
            let location = object
                .get("location")
                .or_else(|| object.get("path"))
                .and_then(serde_json::Value::as_str);
            if let (Some(name), Some(location)) = (name, location) {
                locations.insert(name.to_string(), location.to_string());
            }
            if let Some(skills) = object.get("skills") {
                collect_debug_skill_locations(skills, None, locations);
            }
            for (key, value) in object {
                if matches!(key.as_str(), "name" | "location" | "path" | "skills") {
                    continue;
                }
                if value.is_object() || value.is_array() {
                    collect_debug_skill_locations(value, Some(key), locations);
                }
            }
        }
        _ => {}
    }
}

fn debug_skill_locations(raw: &str) -> Option<BTreeMap<String, String>> {
    let value = serde_json::from_str(raw).ok()?;
    let mut locations = BTreeMap::new();
    collect_debug_skill_locations(&value, None, &mut locations);
    Some(locations)
}

pub(crate) fn resolve_sources(scan_root: &Path, sources: &mut [ShadowSource]) {
    let has_duplicate_runtime_id = sources.iter().enumerate().any(|(index, source)| {
        sources[index + 1..]
            .iter()
            .any(|other| other.runtime_id == source.runtime_id)
    });
    if !has_duplicate_runtime_id {
        crate::adapters::skill_shadow::resolve_from_selected_paths(sources, &BTreeMap::new());
        return;
    }

    let locations = Command::new("opencode")
        .args(["debug", "skill"])
        .current_dir(scan_root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|raw| debug_skill_locations(&raw))
        .unwrap_or_default();
    crate::adapters::skill_shadow::resolve_from_selected_paths(sources, &locations);
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
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn write_skill(path: &Path, name: &str) {
        fs::create_dir_all(path).unwrap();
        fs::write(
            path.join("SKILL.md"),
            format!("---\nname: '{name}'\ndescription: test\n---\n"),
        )
        .unwrap();
    }

    /// A git worktree (`repo/.git`) with a staged env nested inside it; the
    /// shadow scan runs against the env root, exactly as `run` invokes it.
    fn repo_with_env(tmp: &Path) -> (PathBuf, PathBuf) {
        let repo = tmp.join("repo");
        let scan_root = repo.join(".eval-magic/skill/iteration-1/env-g1-with_skill");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(&scan_root).unwrap();
        (repo, scan_root)
    }

    fn detect(scan_root: &Path, home: &Path) -> PluginShadowReport {
        detect_with_sources(scan_root, &["target-skill"], home, None, None)
    }

    fn assert_source_path_ends(report: &PluginShadowReport, suffix: &str) {
        assert_eq!(report.source_count(), 1);
        assert_eq!(report.source(0).kind, ShadowSourceKind::Skill);
        assert!(
            report.source(0).discovery_path.ends_with(suffix),
            "{:?}",
            report.source(0)
        );
    }

    #[test]
    fn project_opencode_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let (repo, scan_root) = repo_with_env(tmp.path());
        write_skill(&repo.join(".opencode/skills/live-copy"), "target-skill");

        let report = detect(&scan_root, &tmp.path().join("home"));

        assert_source_path_ends(&report, "live-copy");
        assert_eq!(report.source(0).root.namespace, ShadowNamespace::Opencode);
        assert_eq!(report.source(0).root.scope, ShadowRootScope::Project);
    }

    #[test]
    fn project_claude_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let (repo, scan_root) = repo_with_env(tmp.path());
        write_skill(&repo.join(".claude/skills/live-copy"), "target-skill");

        let report = detect(&scan_root, &tmp.path().join("home"));

        assert_source_path_ends(&report, "live-copy");
        assert_eq!(report.source(0).root.namespace, ShadowNamespace::Claude);
        assert_eq!(report.source(0).root.relation, ShadowRelation::CrossHarness);
    }

    #[test]
    fn project_agents_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let (repo, scan_root) = repo_with_env(tmp.path());
        write_skill(&repo.join(".agents/skills/live-copy"), "target-skill");

        let report = detect(&scan_root, &tmp.path().join("home"));

        assert_source_path_ends(&report, "live-copy");
        assert_eq!(report.source(0).root.namespace, ShadowNamespace::Agents);
    }

    #[test]
    fn global_opencode_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write_skill(
            &home.join(".config/opencode/skills/live-copy"),
            "target-skill",
        );

        let report = detect(tmp.path(), &home);

        assert_source_path_ends(&report, "live-copy");
        assert_eq!(report.source(0).root.scope, ShadowRootScope::Global);
    }

    #[test]
    fn global_claude_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write_skill(&home.join(".claude/skills/live-copy"), "target-skill");

        let report = detect(tmp.path(), &home);

        assert_source_path_ends(&report, "live-copy");
        assert_eq!(report.source(0).root.namespace, ShadowNamespace::Claude);
    }

    #[test]
    fn global_agents_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write_skill(&home.join(".agents/skills/live-copy"), "target-skill");

        let report = detect(tmp.path(), &home);

        assert_source_path_ends(&report, "live-copy");
        assert_eq!(report.source(0).root.namespace, ShadowNamespace::Agents);
    }

    #[test]
    fn legacy_home_opencode_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write_skill(&home.join(".opencode/skills/live-copy"), "target-skill");

        let report = detect(tmp.path(), &home);

        assert_source_path_ends(&report, "live-copy");
        assert_eq!(report.source(0).root.namespace, ShadowNamespace::Opencode);
    }

    #[test]
    fn ancestor_walk_is_capped_at_the_git_worktree() {
        let tmp = TempDir::new().unwrap();
        let (repo, scan_root) = repo_with_env(tmp.path());
        // Above the worktree (tmp has no .git): invisible to the walk.
        write_skill(
            &tmp.path().join(".claude/skills/above-worktree"),
            "target-skill",
        );
        write_skill(&repo.join(".claude/skills/inside-worktree"), "target-skill");

        let report = detect(&scan_root, &tmp.path().join("home"));

        assert_source_path_ends(&report, "inside-worktree");
    }

    #[test]
    fn staged_env_root_itself_is_not_scanned() {
        let tmp = TempDir::new().unwrap();
        let (_repo, scan_root) = repo_with_env(tmp.path());
        // The env's own staged copy is intentional, not contamination.
        write_skill(
            &scan_root.join(".opencode/skills/staged-copy"),
            "target-skill",
        );

        let report = detect(&scan_root, &tmp.path().join("home"));

        assert!(report.is_empty());
    }

    #[test]
    fn opencode_config_dir_is_scanned_additively() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let override_dir = tmp.path().join("opencode-config");
        write_skill(
            &home.join(".config/opencode/skills/default-copy"),
            "target-skill",
        );
        write_skill(&override_dir.join("skills/override-copy"), "target-skill");

        let report = detect_with_sources(
            tmp.path(),
            &["target-skill"],
            &home,
            None,
            Some(&override_dir),
        );

        assert_eq!(report.source_count(), 2);
    }

    #[test]
    fn xdg_config_home_redirects_the_default_global_dir() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let xdg = tmp.path().join("xdg");
        write_skill(&xdg.join("opencode/skills/xdg-copy"), "target-skill");
        // With XDG_CONFIG_HOME set, the non-xdg default is not scanned.
        write_skill(
            &home.join(".config/opencode/skills/non-xdg-copy"),
            "target-skill",
        );

        let report = detect_with_sources(tmp.path(), &["target-skill"], &home, Some(&xdg), None);

        assert_source_path_ends(&report, "xdg-copy");
    }

    #[test]
    fn repeated_roots_are_reported_once() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let default = home.join(".config/opencode");
        write_skill(&default.join("skills/live-copy"), "target-skill");

        // OPENCODE_CONFIG_DIR pointing at the default dir must not double-report.
        let report =
            detect_with_sources(tmp.path(), &["target-skill"], &home, None, Some(&default));

        assert_eq!(report.source_count(), 1);
    }

    #[test]
    fn direct_scan_uses_frontmatter_name_not_folder_name() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write_skill(
            &home.join(".agents/skills/different-folder"),
            "target-skill",
        );

        let report = detect(tmp.path(), &home);

        assert_eq!(report.source_count(), 1);
        assert_eq!(report.source(0).skill_name(), "target-skill");
    }

    #[test]
    fn malformed_skills_do_not_create_false_reports() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let no_frontmatter = home.join(".agents/skills/plain");
        fs::create_dir_all(&no_frontmatter).unwrap();
        fs::write(no_frontmatter.join("SKILL.md"), "name: target-skill\n").unwrap();
        let unclosed = home.join(".agents/skills/unclosed");
        fs::create_dir_all(&unclosed).unwrap();
        fs::write(unclosed.join("SKILL.md"), "---\nname: target-skill\n").unwrap();

        let report = detect(tmp.path(), &home);

        assert!(report.is_empty());
    }

    #[test]
    fn non_staged_skill_names_are_not_reported() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write_skill(&home.join(".agents/skills/live-copy"), "some-other-skill");

        let report = detect(tmp.path(), &home);

        assert!(report.is_empty());
    }

    fn sample_report() -> PluginShadowReport {
        let path = Path::new("/home/u/.claude/skills/target-skill");
        PluginShadowReport::from_sources(
            "/home/u/.config/opencode",
            vec![ShadowSource::live_skill(
                "target-skill",
                path,
                ShadowRoot {
                    scope: ShadowRootScope::Global,
                    namespace: ShadowNamespace::Claude,
                    plugin: None,
                    path: "/home/u/.claude/skills".into(),
                    relation: ShadowRelation::CrossHarness,
                },
                "Set OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1 for every dispatch.",
            )],
        )
    }

    #[test]
    fn banner_is_empty_when_nothing_shadowed() {
        let empty = PluginShadowReport {
            config_dir: "/x".into(),
            findings: vec![],
        };
        assert_eq!(format_shadow_banner(&empty), "");
    }

    #[test]
    fn banner_lists_findings_remediation_and_isolation_doc() {
        let banner = format_shadow_banner(&sample_report());
        assert!(banner.contains("target-skill"), "{banner}");
        assert!(banner.contains(".claude/skills"), "{banner}");
        assert!(
            banner.contains("OPENCODE_DISABLE_CLAUDE_CODE_SKILLS"),
            "banner names the .claude-root kill switch: {banner}"
        );
        assert!(banner.contains("cross-harness"), "{banner}");
        // Pre-dispatch the banner states the stake in the conditional, not a
        // verdict: nothing has run, so nothing is invalid yet.
        assert!(
            banner.contains("would invalidate the comparison if loaded"),
            "{banner}"
        );
        assert!(!banner.contains("[comparison invalid]"), "{banner}");
    }

    #[test]
    fn validity_warnings_name_skill_source_contamination_and_doc() {
        let warnings = shadow_validity_warnings(&sample_report());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("target-skill"));
        assert!(warnings[0].contains(".claude/skills"));
        assert!(warnings[0].contains("comparison invalid"));
        assert!(warnings[0].contains("OPENCODE_DISABLE_CLAUDE_CODE_SKILLS"));
    }

    #[test]
    fn debug_skill_output_maps_runtime_ids_to_selected_locations() {
        let locations = debug_skill_locations(
            r#"[
              {"name":"helper","location":"/repo/.agents/skills/helper/SKILL.md"},
              {"name":"other","location":"/repo/.opencode/skills/other/SKILL.md"}
            ]"#,
        )
        .unwrap();

        assert_eq!(
            locations.get("helper").map(String::as_str),
            Some("/repo/.agents/skills/helper/SKILL.md")
        );
    }
}
