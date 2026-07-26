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

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::adapters::skill_shadow::{PluginShadowReport, ShadowSource};

const ISOLATION_DOC: &str = "docs/opencode-notes.md → \"Isolating from live skills\"";

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

fn direct_skill_sources(dir: &Path) -> Vec<ShadowSource> {
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
            Some(ShadowSource::GlobalSkill {
                skill_name,
                path: path.to_string_lossy().into_owned(),
            })
        })
        .collect()
}

/// The three project skill dirs OpenCode checks at each level of the
/// up-from-cwd walk.
const PROJECT_SKILL_DIRS: [&str; 3] = [".opencode/skills", ".claude/skills", ".agents/skills"];

fn repository_skill_dirs(scan_root: &Path) -> Vec<PathBuf> {
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
        for sub in PROJECT_SKILL_DIRS {
            dirs.push(path.join(sub));
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
) -> Vec<PathBuf> {
    let mut dirs = vec![default_config_dir(home, xdg_config_home).join("skills")];
    if let Some(dir) = opencode_config_dir {
        dirs.push(dir.join("skills"));
    }
    dirs.push(home.join(".opencode/skills"));
    dirs.push(home.join(".claude/skills"));
    dirs.push(home.join(".agents/skills"));
    dirs
}

fn default_config_dir(home: &Path, xdg_config_home: Option<&Path>) -> PathBuf {
    xdg_config_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".config"))
        .join("opencode")
}

fn sort_and_dedup(sources: &mut Vec<ShadowSource>) {
    sources.sort_by_key(|source| match source {
        ShadowSource::Plugin {
            plugin,
            skill_name,
            path,
        } => format!("plugin\0{plugin}\0{skill_name}\0{path}"),
        ShadowSource::GlobalSkill { skill_name, path } => {
            format!("skill\0{skill_name}\0{path}")
        }
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
    for dir in dirs {
        if seen_dirs.insert(dir.clone()) {
            shadowed.extend(direct_skill_sources(&dir));
        }
    }
    shadowed.retain(|source| staged.contains(source.skill_name()));
    sort_and_dedup(&mut shadowed);

    PluginShadowReport {
        config_dir: default_config_dir(home, xdg_config_home)
            .to_string_lossy()
            .into_owned(),
        shadowed,
    }
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
    (!report.shadowed.is_empty()).then_some(report)
}

fn source_label(source: &ShadowSource) -> String {
    match source {
        ShadowSource::Plugin { plugin, .. } => format!("enabled plugin '{plugin}'"),
        ShadowSource::GlobalSkill { path, .. } => {
            let parent = Path::new(path).parent().unwrap_or_else(|| Path::new(path));
            format!("skill directory '{}'", parent.display())
        }
    }
}

/// One OpenCode-specific `validity_warnings` line per shadowed skill.
pub fn shadow_validity_warnings(report: &PluginShadowReport) -> Vec<String> {
    report
        .shadowed
        .iter()
        .map(|source| {
            format!(
                "staged skill '{}' is also provided by {} — each `opencode run` dispatch could \
                 discover both copies, so with/without results may be contaminated. Isolate the \
                 live skill source before dispatch (see {}).",
                source.skill_name(),
                source_label(source),
                ISOLATION_DOC
            )
        })
        .collect()
}

/// OpenCode-specific build-time banner. Empty when nothing is shadowed.
pub fn format_shadow_banner(report: &PluginShadowReport) -> String {
    if report.shadowed.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        String::new(),
        "⚠ OpenCode skill-shadow warning: skills staged for this eval are ALSO discoverable"
            .to_string(),
        "  from your live environment:".to_string(),
    ];
    for source in &report.shadowed {
        lines.push(format!(
            "  • {} — {}",
            source.skill_name(),
            source_label(source)
        ));
    }
    lines.extend([
        "  Each `opencode run` dispatch discovers project and global .opencode, .claude,"
            .to_string(),
        "  and .agents skill dirs, so it can load both copies — the with/without".to_string(),
        "  comparison may be contaminated and the control arm may not be skill-absent.".to_string(),
        "  eval-magic cannot unload a live skill. Before dispatch:".to_string(),
        "  1. Move or rename the conflicting skill directory.".to_string(),
        "  2. For a .claude-root collision only: run the dispatch with".to_string(),
        "     OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1.".to_string(),
        "  3. For a .claude or .agents collision: OPENCODE_DISABLE_EXTERNAL_SKILLS=1".to_string(),
        "     hides both cross-harness roots.".to_string(),
        format!("  Full mechanics and detection limits: {ISOLATION_DOC}."),
    ]);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::skill_shadow::ShadowSource;
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

    #[test]
    fn project_opencode_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let (repo, scan_root) = repo_with_env(tmp.path());
        write_skill(&repo.join(".opencode/skills/live-copy"), "target-skill");

        let report = detect(&scan_root, &tmp.path().join("home"));

        assert_eq!(report.shadowed.len(), 1);
        assert!(
            matches!(&report.shadowed[0], ShadowSource::GlobalSkill { path, .. } if path.ends_with("live-copy"))
        );
    }

    #[test]
    fn project_claude_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let (repo, scan_root) = repo_with_env(tmp.path());
        write_skill(&repo.join(".claude/skills/live-copy"), "target-skill");

        let report = detect(&scan_root, &tmp.path().join("home"));

        assert_eq!(report.shadowed.len(), 1);
        assert!(
            matches!(&report.shadowed[0], ShadowSource::GlobalSkill { path, .. } if path.ends_with("live-copy"))
        );
    }

    #[test]
    fn project_agents_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let (repo, scan_root) = repo_with_env(tmp.path());
        write_skill(&repo.join(".agents/skills/live-copy"), "target-skill");

        let report = detect(&scan_root, &tmp.path().join("home"));

        assert_eq!(report.shadowed.len(), 1);
        assert!(
            matches!(&report.shadowed[0], ShadowSource::GlobalSkill { path, .. } if path.ends_with("live-copy"))
        );
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

        assert_eq!(report.shadowed.len(), 1);
        assert!(
            matches!(&report.shadowed[0], ShadowSource::GlobalSkill { path, .. } if path.ends_with("live-copy"))
        );
    }

    #[test]
    fn global_claude_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write_skill(&home.join(".claude/skills/live-copy"), "target-skill");

        let report = detect(tmp.path(), &home);

        assert_eq!(report.shadowed.len(), 1);
        assert!(
            matches!(&report.shadowed[0], ShadowSource::GlobalSkill { path, .. } if path.ends_with("live-copy"))
        );
    }

    #[test]
    fn global_agents_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write_skill(&home.join(".agents/skills/live-copy"), "target-skill");

        let report = detect(tmp.path(), &home);

        assert_eq!(report.shadowed.len(), 1);
        assert!(
            matches!(&report.shadowed[0], ShadowSource::GlobalSkill { path, .. } if path.ends_with("live-copy"))
        );
    }

    #[test]
    fn legacy_home_opencode_root_detects_collision() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write_skill(&home.join(".opencode/skills/live-copy"), "target-skill");

        let report = detect(tmp.path(), &home);

        assert_eq!(report.shadowed.len(), 1);
        assert!(
            matches!(&report.shadowed[0], ShadowSource::GlobalSkill { path, .. } if path.ends_with("live-copy"))
        );
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

        assert_eq!(report.shadowed.len(), 1);
        assert!(
            matches!(&report.shadowed[0], ShadowSource::GlobalSkill { path, .. } if path.ends_with("inside-worktree"))
        );
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

        assert!(report.shadowed.is_empty());
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

        assert_eq!(report.shadowed.len(), 2);
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

        assert_eq!(report.shadowed.len(), 1);
        assert!(
            matches!(&report.shadowed[0], ShadowSource::GlobalSkill { path, .. } if path.ends_with("xdg-copy"))
        );
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

        assert_eq!(report.shadowed.len(), 1);
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

        assert_eq!(report.shadowed.len(), 1);
        assert_eq!(report.shadowed[0].skill_name(), "target-skill");
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

        assert!(report.shadowed.is_empty());
    }

    #[test]
    fn non_staged_skill_names_are_not_reported() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        write_skill(&home.join(".agents/skills/live-copy"), "some-other-skill");

        let report = detect(tmp.path(), &home);

        assert!(report.shadowed.is_empty());
    }

    fn sample_report() -> PluginShadowReport {
        PluginShadowReport {
            config_dir: "/home/u/.config/opencode".into(),
            shadowed: vec![ShadowSource::GlobalSkill {
                skill_name: "target-skill".into(),
                path: "/home/u/.claude/skills/target-skill".into(),
            }],
        }
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
    fn banner_lists_findings_remediation_and_isolation_doc() {
        let banner = format_shadow_banner(&sample_report());
        assert!(banner.contains("target-skill"), "{banner}");
        assert!(banner.contains(".claude/skills"), "{banner}");
        assert!(banner.contains("opencode run"), "{banner}");
        assert!(
            banner.contains("OPENCODE_DISABLE_CLAUDE_CODE_SKILLS"),
            "banner names the .claude-root kill switch: {banner}"
        );
        assert!(
            banner.contains("OPENCODE_DISABLE_EXTERNAL_SKILLS"),
            "banner names the external-roots kill switch: {banner}"
        );
        assert!(banner.contains("docs/opencode-notes.md"), "{banner}");
    }

    #[test]
    fn validity_warnings_name_skill_source_contamination_and_doc() {
        let warnings = shadow_validity_warnings(&sample_report());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("target-skill"));
        assert!(warnings[0].contains(".claude/skills"));
        assert!(warnings[0].contains("opencode run"));
        assert!(warnings[0].to_lowercase().contains("contaminat"));
        assert!(warnings[0].contains("docs/opencode-notes.md"));
    }
}
