//! Live-skill shadow detector and reporting for Cline.
//!
//! Cline discovers skills from exactly three roots (verified against 3.0.53
//! with live dispatches — see docs/cline-notes.md): the dispatch cwd's
//! `.cline/skills` (runner-controlled staging during evals, so never a
//! contamination source — Cline does **not** walk project ancestors), the
//! global `$CLINE_DIR/skills` (default `~/.cline/skills`), and the
//! cross-harness `~/.agents/skills` (where `cline skill install` lands global
//! installs). A logical eval skill present in either global root contaminates
//! the control arm even though eval-magic stages its test copy under a
//! generated slug. Detection is deliberately best-effort: missing directories
//! and unreadable/malformed skills are ignored while all roots continue to be
//! scanned.

use std::fs;
use std::path::{Path, PathBuf};

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

/// The Cline state dir: `$CLINE_DIR` when set, else `~/.cline` (3.0.53 binary:
/// `process.env.CLINE_DIR?.trim() || join(homedir(), ".cline")`).
fn cline_dir(home: &Path) -> PathBuf {
    env_path("CLINE_DIR").unwrap_or_else(|| home.join(".cline"))
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

/// Read the top-level `name:` from a skill's YAML frontmatter. Cline keys
/// discovery by this value (the docs require it to match the directory name).
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

/// List the skills one root dir contributes, keyed by frontmatter name.
fn direct_skill_sources(
    dir: &Path,
    namespace: ShadowNamespace,
    relation: ShadowRelation,
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
                    scope: ShadowRootScope::Global,
                    namespace,
                    plugin: None,
                    path: dir.to_string_lossy().into_owned(),
                    relation,
                },
                format!(
                    "Move or rename the conflicting skill directory '{}'.",
                    path.display()
                ),
            ))
        })
        .collect()
}

fn detect_with_sources(
    staged_skill_names: &[&str],
    cline_dir: &Path,
    home: &Path,
) -> PluginShadowReport {
    let staged: std::collections::HashSet<&str> = staged_skill_names.iter().copied().collect();
    let mut sources = Vec::new();
    for (dir, namespace, relation) in [
        (
            cline_dir.join("skills"),
            ShadowNamespace::Cline,
            ShadowRelation::Native,
        ),
        (
            home.join(".agents/skills"),
            ShadowNamespace::Agents,
            ShadowRelation::CrossHarness,
        ),
    ] {
        sources.extend(
            direct_skill_sources(&dir, namespace, relation)
                .into_iter()
                .filter(|source| staged.contains(source.skill_name())),
        );
    }
    sources.sort_by(|a, b| {
        (&a.plugin, &a.skill_name, &a.discovery_path).cmp(&(
            &b.plugin,
            &b.skill_name,
            &b.discovery_path,
        ))
    });
    sources.dedup();
    PluginShadowReport::from_sources(cline_dir.to_string_lossy(), sources)
}

/// Detect logical eval skill names that Cline can also load from live global
/// sources. `scan_root` is accepted for interface uniformity and intentionally
/// unused: Cline reads only the dispatch cwd's `.cline/skills` (verified
/// against 3.0.53 — no ancestor walk), and during evals that cwd is the
/// runner-controlled staged env.
pub fn shadow_preflight(
    _scan_root: &Path,
    staged_skill_names: &[&str],
) -> Option<PluginShadowReport> {
    let home = user_home();
    let report = detect_with_sources(staged_skill_names, &cline_dir(&home), &home);
    (!report.is_empty()).then_some(report)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::detect_with_sources;
    use crate::adapters::skill_shadow::{
        ShadowNamespace, ShadowRelation, ShadowRootScope, ShadowSourceKind,
    };

    fn write_skill(root: &std::path::Path, dir_name: &str, frontmatter_name: &str) {
        let dir = root.join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {frontmatter_name}\ndescription: x\n---\n\nbody\n"),
        )
        .unwrap();
    }

    /// A live copy under the Cline global root shadows the staged slug's
    /// logical skill; the report names the native Cline namespace.
    #[test]
    fn detects_a_live_global_cline_skill_shadowing_a_staged_name() {
        let tmp = TempDir::new().unwrap();
        let cline_dir = tmp.path().join("cline-home");
        let home = tmp.path().join("home");
        write_skill(&cline_dir.join("skills"), "mr-review", "mr-review");

        let report = detect_with_sources(&["mr-review"], &cline_dir, &home);

        assert_eq!(report.source_count(), 1);
        let source = report.source(0);
        assert_eq!(source.kind, ShadowSourceKind::Skill);
        assert_eq!(source.root.namespace, ShadowNamespace::Cline);
        assert_eq!(source.root.scope, ShadowRootScope::Global);
        assert_eq!(source.root.relation, ShadowRelation::Native);
        assert!(
            source
                .remediation
                .as_deref()
                .unwrap_or_default()
                .contains("ove or rename"),
            "{source:?}"
        );
    }

    /// The cross-harness `~/.agents/skills` root is read at runtime (3.0.53
    /// probe), so a Codex-installed skill there shadows too — reported under
    /// the Agents namespace as cross-harness.
    #[test]
    fn detects_a_live_agents_skill_as_cross_harness() {
        let tmp = TempDir::new().unwrap();
        let cline_dir = tmp.path().join("cline-home");
        let home = tmp.path().join("home");
        write_skill(&home.join(".agents/skills"), "mr-review", "mr-review");

        let report = detect_with_sources(&["mr-review"], &cline_dir, &home);

        assert_eq!(report.source_count(), 1);
        let source = report.source(0);
        assert_eq!(source.root.namespace, ShadowNamespace::Agents);
        assert_eq!(source.root.relation, ShadowRelation::CrossHarness);
    }

    /// Discovery keys on the frontmatter `name:`, not the directory name
    /// (Cline requires them to match, but the frontmatter wins); unrelated
    /// names and malformed skills never report.
    #[test]
    fn keys_on_frontmatter_names_and_ignores_unrelated_or_malformed_skills() {
        let tmp = TempDir::new().unwrap();
        let cline_dir = tmp.path().join("cline-home");
        let home = tmp.path().join("home");
        // Directory name differs from frontmatter name: frontmatter wins.
        write_skill(&cline_dir.join("skills"), "renamed-dir", "mr-review");
        // Unrelated skill: never reported.
        write_skill(&cline_dir.join("skills"), "other", "other");
        // Malformed (no frontmatter block): ignored, not a false positive.
        let malformed = home.join(".agents/skills/mr-review");
        fs::create_dir_all(&malformed).unwrap();
        fs::write(malformed.join("SKILL.md"), "name: mr-review\n").unwrap();

        let report = detect_with_sources(&["mr-review"], &cline_dir, &home);

        assert_eq!(report.source_count(), 1);
        assert_eq!(report.source(0).skill_name, "mr-review");
        assert!(report.source(0).discovery_path.contains("renamed-dir"));
    }
}
