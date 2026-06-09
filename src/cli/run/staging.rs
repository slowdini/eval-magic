//! Staged-skill lifecycle: install a skill (and its siblings) into the harness's
//! project-local skills dir so eval subagents can discover it, and tear that
//! staging back down — restoring any pre-existing skills the runner displaced.
//!
//! Ports `run.ts:51-331`. The sibling-staging manifest
//! (`.slow-powers-eval-manifest.json`) records what the runner created and what
//! it backed up, so [`cleanup_staged_skills`] can surgically undo only its own
//! changes and leave the user's own project skills intact.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::core::Harness;
use crate::pipeline::io::now_iso8601;
use crate::workspace::SNAPSHOT_META;

use super::{RunError, copy_dir_recursive, copy_entry, write_json};

/// Prefix for the conspicuous staged-skill slug. The prefix scan in
/// [`cleanup_staged_skills`] keys on it to remove staged dirs.
pub const STAGED_SKILL_PREFIX: &str = "slow-powers-eval-";

/// Filename of the sibling-staging manifest written under the harness skills dir.
pub const STAGED_SIBLING_MANIFEST: &str = ".slow-powers-eval-manifest.json";

/// One entry in a [`SiblingManifest`]: a dir the runner created, whether it
/// displaced a pre-existing entry, and (if so) where the original was backed up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatedEntry {
    pub name: String,
    pub preexisting: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
}

/// Bookkeeping written by [`stage_sibling_skills`] so cleanup can be surgical.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiblingManifest {
    pub created_at: String,
    pub staged_under_test: String,
    /// Whether the harness skills dir already existed when staging began. `false`
    /// → the runner created it, so cleanup may remove the whole tree and prune an
    /// emptied parent; `true`/absent → surgical per-entry restore.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills_dir_preexisting: Option<bool>,
    pub created_entries: Vec<CreatedEntry>,
}

/// Options for staging a single skill. `harness` defaults to Claude Code via
/// [`Default`]; [`stage_skill_for_cc`] is the convenience wrapper for it.
#[derive(Debug, Clone)]
pub struct StageSkillOpts<'a> {
    pub content: &'a str,
    pub iteration: u32,
    pub condition: &'a str,
    pub skill_name: &'a str,
    pub repo_root: &'a Path,
    /// Source skill dir whose sibling assets are copied alongside the staged
    /// `SKILL.md` (everything but `SKILL.md`, `evals/`, and the snapshot meta).
    pub assets_dir: Option<&'a Path>,
    /// Stage under this verbatim identifier instead of the `slow-powers-eval-…`
    /// slug. Not caught by the prefix scan, so the caller must also call
    /// [`register_staged_skill_for_cleanup`].
    pub stage_name_override: Option<&'a str>,
    pub harness: Harness,
}

impl Default for StageSkillOpts<'_> {
    fn default() -> Self {
        Self {
            content: "",
            iteration: 0,
            condition: "",
            skill_name: "",
            repo_root: Path::new(""),
            assets_dir: None,
            stage_name_override: None,
            harness: Harness::ClaudeCode,
        }
    }
}

/// Options for staging the non-test sibling skills discoverable to an eval.
#[derive(Debug, Clone)]
pub struct StageSiblingOpts<'a> {
    pub skill_under_test: &'a str,
    pub skills_source_dir: &'a Path,
    pub repo_root: &'a Path,
    pub harness: Harness,
}

impl Default for StageSiblingOpts<'_> {
    fn default() -> Self {
        Self {
            skill_under_test: "",
            skills_source_dir: Path::new(""),
            repo_root: Path::new(""),
            harness: Harness::ClaudeCode,
        }
    }
}

/// `<repo_root>/.agents/skills` (Codex) or `<repo_root>/.claude/skills`.
pub(crate) fn skills_dir_for_harness(repo_root: &Path, harness: Harness) -> PathBuf {
    match harness {
        Harness::Codex => repo_root.join(".agents").join("skills"),
        Harness::ClaudeCode => repo_root.join(".claude").join("skills"),
    }
}

/// Rewrite (or insert) the `name:` frontmatter field so a Codex-staged skill's
/// declared name matches its staged slug. Ports `run.ts:122-138`.
fn rewrite_frontmatter_name(content: &str, name: &str) -> String {
    if !content.starts_with("---") {
        return format!("---\nname: {name}\ndescription: Staged eval skill.\n---\n\n{content}");
    }
    let end = content[3..].find("\n---").map(|i| i + 3);
    let Some(end) = end else {
        return content.replacen("---\n", &format!("---\nname: {name}\n"), 1);
    };
    let frontmatter = &content[..end];
    let rest = &content[end..];
    if Regex::new(r"(?m)^name\s*:").unwrap().is_match(frontmatter) {
        let rewritten = Regex::new(r"(?m)^name\s*:.*$")
            .unwrap()
            .replace(frontmatter, format!("name: {name}").as_str());
        format!("{rewritten}{rest}")
    } else {
        content.replacen("---\n", &format!("---\nname: {name}\n"), 1)
    }
}

/// Remove `dir` only if it exists and is empty — prunes a harness config dir the
/// runner emptied without touching one that still holds the user's files.
fn prune_if_empty(dir: &Path) -> Result<(), RunError> {
    if dir.exists() && fs::read_dir(dir)?.next().is_none() {
        fs::remove_dir_all(dir)?;
    }
    Ok(())
}

/// Stage one skill under the harness's skills dir and return its slug. For Codex
/// the frontmatter `name:` is rewritten to the slug. Ports `run.ts:140-164`.
pub fn stage_skill_for_harness(opts: &StageSkillOpts) -> Result<String, RunError> {
    let slug = match opts.stage_name_override {
        Some(name) => name.to_string(),
        None => format!(
            "{STAGED_SKILL_PREFIX}{}-{}__{}",
            opts.iteration, opts.condition, opts.skill_name
        ),
    };
    let skill_dir = skills_dir_for_harness(opts.repo_root, opts.harness).join(&slug);
    fs::create_dir_all(&skill_dir)?;

    let content = if opts.harness == Harness::Codex {
        rewrite_frontmatter_name(opts.content, &slug)
    } else {
        opts.content.to_string()
    };
    fs::write(skill_dir.join("SKILL.md"), content)?;

    if let Some(assets_dir) = opts.assets_dir
        && assets_dir.exists()
    {
        for entry in fs::read_dir(assets_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == "SKILL.md" || name == "evals" || name == SNAPSHOT_META {
                continue;
            }
            copy_entry(&assets_dir.join(&name), &skill_dir.join(&name))?;
        }
    }
    Ok(slug)
}

/// Stage a skill for Claude Code (`.claude/skills`). Convenience wrapper over
/// [`stage_skill_for_harness`] — the orchestrator always passes an explicit
/// harness, so this mirrors eval-runner's `stageSkillForCC` for the tests.
#[cfg(test)]
pub fn stage_skill_for_cc(opts: &StageSkillOpts) -> Result<String, RunError> {
    stage_skill_for_harness(&StageSkillOpts {
        harness: Harness::ClaudeCode,
        ..opts.clone()
    })
}

/// Record a custom-named staged dir (one created via `stage_name_override`) in
/// the sibling manifest so the next run's [`cleanup_staged_skills`] removes it —
/// the prefix scan only catches `slow-powers-eval-…`. Idempotent.
/// Ports `run.ts:176-197`.
pub fn register_staged_skill_for_cleanup(
    repo_root: &Path,
    name: &str,
    harness: Harness,
) -> Result<(), RunError> {
    let manifest_path = skills_dir_for_harness(repo_root, harness).join(STAGED_SIBLING_MANIFEST);
    let mut manifest: SiblingManifest = if manifest_path.exists() {
        serde_json::from_str(&fs::read_to_string(&manifest_path)?)?
    } else {
        SiblingManifest {
            created_at: now_iso8601(),
            staged_under_test: name.to_string(),
            skills_dir_preexisting: Some(true),
            created_entries: Vec::new(),
        }
    };
    if manifest.created_entries.iter().any(|e| e.name == name) {
        return Ok(());
    }
    manifest.created_entries.push(CreatedEntry {
        name: name.to_string(),
        preexisting: false,
        backup_path: None,
    });
    write_json(&manifest_path, &manifest)
}

/// Stage every non-test sibling skill (each `<name>/` with a `SKILL.md`, minus
/// its `evals/`) into the harness skills dir, backing up any colliding
/// pre-existing entry, and write the manifest. Ports `run.ts:217-276`.
pub fn stage_sibling_skills(opts: &StageSiblingOpts) -> Result<SiblingManifest, RunError> {
    let skills_dir = skills_dir_for_harness(opts.repo_root, opts.harness);
    let skills_dir_preexisting = skills_dir.exists();
    fs::create_dir_all(&skills_dir)?;

    let mut siblings: Vec<String> = Vec::new();
    for entry in fs::read_dir(opts.skills_source_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == opts.skill_under_test {
            continue;
        }
        let src_dir = opts.skills_source_dir.join(&name);
        if !src_dir.is_dir() || !src_dir.join("SKILL.md").exists() {
            continue;
        }
        siblings.push(name);
    }
    siblings.sort();

    let mut manifest = SiblingManifest {
        created_at: now_iso8601(),
        staged_under_test: opts.skill_under_test.to_string(),
        skills_dir_preexisting: Some(skills_dir_preexisting),
        created_entries: Vec::new(),
    };

    for name in siblings {
        let src_dir = opts.skills_source_dir.join(&name);
        let dst_dir = skills_dir.join(&name);
        let mut entry = CreatedEntry {
            name: name.clone(),
            preexisting: false,
            backup_path: None,
        };

        if dst_dir.exists() {
            entry.preexisting = true;
            // mkdtemp-style persistent backup dir (must outlive this call so
            // cleanup can restore from it). Built without the dev-only `tempfile`
            // crate so it stays out of the shipped binary, mirroring the TS
            // `mkdtempSync(tmpdir(), "slow-powers-eval-backup-")`.
            let backup_root = make_backup_root()?;
            let backup_path = backup_root.join(&name);
            copy_dir_recursive(&dst_dir, &backup_path)?;
            fs::remove_dir_all(&dst_dir)?;
            entry.backup_path = Some(backup_path.display().to_string());
        }

        // Copy the source skill minus its `evals/` subdir.
        fs::create_dir_all(&dst_dir)?;
        for child in fs::read_dir(&src_dir)? {
            let child = child?;
            if child.file_name() == "evals" {
                continue;
            }
            copy_entry(&child.path(), &dst_dir.join(child.file_name()))?;
        }

        manifest.created_entries.push(entry);
    }

    write_json(&skills_dir.join(STAGED_SIBLING_MANIFEST), &manifest)?;
    Ok(manifest)
}

/// Remove the staged skills (prefix-scanned + manifest-listed) and restore any
/// pre-existing siblings the runner displaced. Ports `run.ts:287-331`.
pub fn cleanup_staged_skills(repo_root: &Path, harness: Harness) -> Result<(), RunError> {
    let harness_dir = match harness {
        Harness::Codex => repo_root.join(".agents"),
        Harness::ClaudeCode => repo_root.join(".claude"),
    };
    let skills_dir = harness_dir.join("skills");
    if !skills_dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(&skills_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(STAGED_SKILL_PREFIX) {
            continue;
        }
        remove_path(&skills_dir.join(&name))?;
    }

    let manifest_path = skills_dir.join(STAGED_SIBLING_MANIFEST);
    if !manifest_path.exists() {
        return Ok(());
    }
    let manifest: SiblingManifest = match fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(m) => m,
        None => {
            let _ = fs::remove_file(&manifest_path);
            return Ok(());
        }
    };

    // The runner created the harness skills dir this run, so it holds none of the
    // user's own skills — remove the whole staged tree (including any stray,
    // non-prefixed dirs left behind), then prune an emptied parent.
    if manifest.skills_dir_preexisting == Some(false) {
        fs::remove_dir_all(&skills_dir)?;
        prune_if_empty(&harness_dir)?;
        return Ok(());
    }

    for e in &manifest.created_entries {
        let target = skills_dir.join(&e.name);
        if target.exists() {
            remove_path(&target)?;
        }
        if e.preexisting
            && let Some(backup) = e.backup_path.as_deref().map(Path::new)
            && backup.exists()
        {
            copy_dir_recursive(backup, &target)?;
            if let Some(parent) = backup.parent() {
                fs::remove_dir_all(parent)?;
            }
        }
    }
    fs::remove_file(&manifest_path)?;
    Ok(())
}

/// Create a fresh, uniquely-named backup dir under the system temp dir, retrying
/// on the (very unlikely) name collision. `create_dir` is atomic enough to claim
/// the name. Replaces the TS `mkdtempSync` without a runtime dependency.
fn make_backup_root() -> Result<PathBuf, RunError> {
    let base = std::env::temp_dir();
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let candidate = base.join(format!(
            "slow-powers-eval-backup-{}-{:06x}",
            now.as_nanos(),
            now.subsec_nanos() & 0x00ff_ffff
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
}

/// Remove a path whether it is a file or a directory.
fn remove_path(path: &Path) -> Result<(), RunError> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    fn read_manifest(skills_dir: &Path) -> SiblingManifest {
        serde_json::from_str(&read(&skills_dir.join(STAGED_SIBLING_MANIFEST))).unwrap()
    }

    /// `<root>/src-skills` with alpha (+evals +helper), beta, gamma.
    fn build_source_skills(root: &Path) -> std::path::PathBuf {
        let src = root.join("src-skills");
        write(&src.join("alpha").join("SKILL.md"), "alpha content");
        write(&src.join("alpha").join("helper.md"), "alpha helper");
        write(&src.join("alpha").join("evals").join("evals.json"), "{}");
        write(&src.join("beta").join("SKILL.md"), "beta content");
        write(&src.join("gamma").join("SKILL.md"), "gamma content");
        src
    }

    // ── stage_skill_for_cc ────────────────────────────────────────────────

    #[test]
    fn writes_skill_md_and_returns_slug() {
        let tmp = TempDir::new().unwrap();
        let content = "---\nname: example\ndescription: example skill\n---\n\nbody\n";
        let slug = stage_skill_for_cc(&StageSkillOpts {
            content,
            iteration: 3,
            condition: "with_skill",
            skill_name: "verification-before-completion",
            repo_root: tmp.path(),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(
            slug,
            "slow-powers-eval-3-with_skill__verification-before-completion"
        );
        let staged = tmp
            .path()
            .join(".claude/skills")
            .join(&slug)
            .join("SKILL.md");
        assert!(staged.exists());
        assert_eq!(read(&staged), content);
    }

    #[test]
    fn overwrites_existing_staged_skill_at_same_slug() {
        let tmp = TempDir::new().unwrap();
        let opts = |content| StageSkillOpts {
            content,
            iteration: 1,
            condition: "with_skill",
            skill_name: "s",
            repo_root: tmp.path(),
            ..Default::default()
        };
        stage_skill_for_cc(&opts("first")).unwrap();
        let slug = stage_skill_for_cc(&opts("second")).unwrap();
        let staged = tmp
            .path()
            .join(".claude/skills")
            .join(&slug)
            .join("SKILL.md");
        assert_eq!(read(&staged), "second");
    }

    #[test]
    fn copies_sibling_assets_from_assets_dir() {
        let tmp = TempDir::new().unwrap();
        let assets = tmp.path().join("assets-src");
        write(&assets.join("SKILL.md"), "the source skill md");
        write(&assets.join("code-review.md"), "review guidance");
        write(
            &assets.join("scripts").join("helper.ts"),
            "export const x = 1",
        );

        let slug = stage_skill_for_cc(&StageSkillOpts {
            content: "staged content",
            iteration: 1,
            condition: "new_skill",
            skill_name: "s",
            repo_root: tmp.path(),
            assets_dir: Some(&assets),
            ..Default::default()
        })
        .unwrap();

        let staged_dir = tmp.path().join(".claude/skills").join(&slug);
        assert_eq!(read(&staged_dir.join("SKILL.md")), "staged content");
        assert_eq!(read(&staged_dir.join("code-review.md")), "review guidance");
        assert_eq!(
            read(&staged_dir.join("scripts").join("helper.ts")),
            "export const x = 1"
        );
    }

    #[test]
    fn excludes_skill_md_evals_and_snapshot_meta_from_asset_copy() {
        let tmp = TempDir::new().unwrap();
        let assets = tmp.path().join("assets-src");
        write(&assets.join("SKILL.md"), "src skill md");
        write(&assets.join("code-review.md"), "keep me");
        write(&assets.join(SNAPSHOT_META), "{\"source\":\"ref\"}");
        write(&assets.join("evals").join("evals.json"), "{}");

        let slug = stage_skill_for_cc(&StageSkillOpts {
            content: "staged",
            iteration: 1,
            condition: "old_skill",
            skill_name: "s",
            repo_root: tmp.path(),
            assets_dir: Some(&assets),
            ..Default::default()
        })
        .unwrap();

        let staged_dir = tmp.path().join(".claude/skills").join(&slug);
        assert!(staged_dir.join("code-review.md").exists());
        assert!(!staged_dir.join("evals").exists());
        assert!(!staged_dir.join(SNAPSHOT_META).exists());
        assert_eq!(read(&staged_dir.join("SKILL.md")), "staged");
    }

    #[test]
    fn stages_skill_md_alone_when_assets_dir_omitted() {
        let tmp = TempDir::new().unwrap();
        let slug = stage_skill_for_cc(&StageSkillOpts {
            content: "solo",
            iteration: 1,
            condition: "with_skill",
            skill_name: "s",
            repo_root: tmp.path(),
            ..Default::default()
        })
        .unwrap();
        let staged_dir = tmp.path().join(".claude/skills").join(&slug);
        let entries: Vec<_> = fs::read_dir(&staged_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["SKILL.md"]);
    }

    #[test]
    fn stage_name_override_stages_under_verbatim_name() {
        let tmp = TempDir::new().unwrap();
        let content = "---\nname: example\ndescription: example skill\n---\n\nbody\n";
        let slug = stage_skill_for_cc(&StageSkillOpts {
            content,
            iteration: 2,
            condition: "with_skill",
            skill_name: "verification-before-completion",
            repo_root: tmp.path(),
            stage_name_override: Some("verification-before-completion"),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(slug, "verification-before-completion");
        let staged = tmp
            .path()
            .join(".claude/skills")
            .join(&slug)
            .join("SKILL.md");
        assert!(staged.exists());
        assert_eq!(read(&staged), content);
    }

    // ── stage_skill_for_harness (codex) ───────────────────────────────────

    #[test]
    fn codex_stages_under_agents_skills_and_rewrites_frontmatter_name() {
        let tmp = TempDir::new().unwrap();
        let content = "---\nname: mr-review\ndescription: review merge requests\n---\n\nbody\n";
        let slug = stage_skill_for_harness(&StageSkillOpts {
            content,
            iteration: 4,
            condition: "with_skill",
            skill_name: "mr-review",
            repo_root: tmp.path(),
            harness: Harness::Codex,
            ..Default::default()
        })
        .unwrap();

        assert_eq!(slug, "slow-powers-eval-4-with_skill__mr-review");
        let staged = tmp
            .path()
            .join(".agents/skills")
            .join(&slug)
            .join("SKILL.md");
        assert!(staged.exists());
        let body = read(&staged);
        assert!(body.contains(&format!("name: {slug}")));
        assert!(body.contains("description: review merge requests"));
        assert!(body.contains("body"));
        assert!(!tmp.path().join(".claude/skills").exists());
    }

    #[test]
    fn codex_stage_name_override_is_dir_and_frontmatter_name() {
        let tmp = TempDir::new().unwrap();
        let slug = stage_skill_for_harness(&StageSkillOpts {
            content: "---\nname: mr-review\ndescription: review merge requests\n---\n\nbody\n",
            iteration: 1,
            condition: "with_skill",
            skill_name: "mr-review",
            repo_root: tmp.path(),
            harness: Harness::Codex,
            stage_name_override: Some("mr-review"),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(slug, "mr-review");
        let staged = read(&tmp.path().join(".agents/skills/mr-review/SKILL.md"));
        assert!(staged.contains("name: mr-review"));
    }

    // ── register_staged_skill_for_cleanup ─────────────────────────────────

    #[test]
    fn register_appends_custom_dir_so_cleanup_removes_it() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join(".claude/skills");
        write(
            &skills_dir.join(STAGED_SIBLING_MANIFEST),
            &serde_json::to_string_pretty(&serde_json::json!({
                "created_at": "x",
                "staged_under_test": "verification-before-completion",
                "created_entries": [{ "name": "sibling-a", "preexisting": false }],
            }))
            .unwrap(),
        );
        let custom_dir = skills_dir.join("verification-before-completion");
        write(&custom_dir.join("SKILL.md"), "staged");

        register_staged_skill_for_cleanup(
            tmp.path(),
            "verification-before-completion",
            Harness::ClaudeCode,
        )
        .unwrap();

        let mut names: Vec<String> = read_manifest(&skills_dir)
            .created_entries
            .iter()
            .map(|e| e.name.clone())
            .collect();
        names.sort();
        assert_eq!(names, vec!["sibling-a", "verification-before-completion"]);

        cleanup_staged_skills(tmp.path(), Harness::ClaudeCode).unwrap();
        assert!(!custom_dir.exists());
    }

    #[test]
    fn register_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join(".claude/skills");
        write(
            &skills_dir.join(STAGED_SIBLING_MANIFEST),
            &serde_json::to_string_pretty(&serde_json::json!({
                "created_at": "x",
                "staged_under_test": "foo",
                "created_entries": [],
            }))
            .unwrap(),
        );

        register_staged_skill_for_cleanup(tmp.path(), "foo-staged", Harness::ClaudeCode).unwrap();
        register_staged_skill_for_cleanup(tmp.path(), "foo-staged", Harness::ClaudeCode).unwrap();

        let count = read_manifest(&skills_dir)
            .created_entries
            .iter()
            .filter(|e| e.name == "foo-staged")
            .count();
        assert_eq!(count, 1);
    }

    // ── cleanup_staged_skills (basic) ─────────────────────────────────────

    #[test]
    fn cleanup_removes_only_prefixed_dirs() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join(".claude/skills");
        let staged_a = skills_dir.join("slow-powers-eval-1-with_skill__foo");
        let staged_b = skills_dir.join("slow-powers-eval-1-new_skill__bar");
        let production = skills_dir.join("user-custom-skill");
        for d in [&staged_a, &staged_b, &production] {
            fs::create_dir_all(d).unwrap();
        }

        cleanup_staged_skills(tmp.path(), Harness::ClaudeCode).unwrap();

        assert!(!staged_a.exists());
        assert!(!staged_b.exists());
        assert!(production.exists());
    }

    #[test]
    fn cleanup_is_noop_when_skills_dir_missing() {
        let tmp = TempDir::new().unwrap();
        cleanup_staged_skills(tmp.path(), Harness::ClaudeCode).unwrap();
    }

    // ── stage_sibling_skills ──────────────────────────────────────────────

    #[test]
    fn stages_each_sibling_minus_evals() {
        let tmp = TempDir::new().unwrap();
        let src = build_source_skills(tmp.path());
        stage_sibling_skills(&StageSiblingOpts {
            skill_under_test: "gamma",
            skills_source_dir: &src,
            repo_root: tmp.path(),
            ..Default::default()
        })
        .unwrap();

        let skills_dir = tmp.path().join(".claude/skills");
        assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "alpha content");
        assert_eq!(read(&skills_dir.join("alpha/helper.md")), "alpha helper");
        assert!(!skills_dir.join("alpha/evals").exists());
        assert_eq!(read(&skills_dir.join("beta/SKILL.md")), "beta content");
        assert!(!skills_dir.join("gamma").exists());

        let mut names: Vec<String> = read_manifest(&skills_dir)
            .created_entries
            .iter()
            .map(|e| e.name.clone())
            .collect();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert!(
            read_manifest(&skills_dir)
                .created_entries
                .iter()
                .all(|e| !e.preexisting)
        );
    }

    #[test]
    fn backs_up_colliding_preexisting_entries() {
        let tmp = TempDir::new().unwrap();
        let src = build_source_skills(tmp.path());
        let skills_dir = tmp.path().join(".claude/skills");
        write(&skills_dir.join("alpha/SKILL.md"), "USER OWNED");

        stage_sibling_skills(&StageSiblingOpts {
            skill_under_test: "gamma",
            skills_source_dir: &src,
            repo_root: tmp.path(),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "alpha content");
        let manifest = read_manifest(&skills_dir);
        let alpha = manifest
            .created_entries
            .iter()
            .find(|e| e.name == "alpha")
            .unwrap();
        assert!(alpha.preexisting);
        let backup = alpha.backup_path.as_deref().unwrap();
        assert!(Path::new(backup).exists());
        assert_eq!(read(&Path::new(backup).join("SKILL.md")), "USER OWNED");
    }

    #[test]
    fn skips_skill_under_test_in_source() {
        let tmp = TempDir::new().unwrap();
        let src = build_source_skills(tmp.path());
        stage_sibling_skills(&StageSiblingOpts {
            skill_under_test: "alpha",
            skills_source_dir: &src,
            repo_root: tmp.path(),
            ..Default::default()
        })
        .unwrap();
        let skills_dir = tmp.path().join(".claude/skills");
        assert!(!skills_dir.join("alpha").exists());
        assert!(skills_dir.join("beta").exists());
        assert!(skills_dir.join("gamma").exists());
    }

    // ── cleanup_staged_skills (manifest-aware) ────────────────────────────

    #[test]
    fn codex_cleanup_restores_preexisting_agents_entries() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src-skills");
        write(&src.join("alpha/SKILL.md"), "new alpha");
        let skills_dir = tmp.path().join(".agents/skills");
        write(&skills_dir.join("alpha/SKILL.md"), "USER ALPHA");

        stage_sibling_skills(&StageSiblingOpts {
            skill_under_test: "x",
            skills_source_dir: &src,
            repo_root: tmp.path(),
            harness: Harness::Codex,
        })
        .unwrap();
        assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "new alpha");

        cleanup_staged_skills(tmp.path(), Harness::Codex).unwrap();

        assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "USER ALPHA");
        assert!(!skills_dir.join(STAGED_SIBLING_MANIFEST).exists());
        assert!(!tmp.path().join(".claude/skills").exists());
    }

    #[test]
    fn removes_sibling_entries_and_restores_backups() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src-skills");
        write(&src.join("alpha/SKILL.md"), "new alpha");
        write(&src.join("beta/SKILL.md"), "new beta");
        let skills_dir = tmp.path().join(".claude/skills");
        write(&skills_dir.join("alpha/SKILL.md"), "USER ALPHA");

        stage_sibling_skills(&StageSiblingOpts {
            skill_under_test: "x",
            skills_source_dir: &src,
            repo_root: tmp.path(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "new alpha");
        assert_eq!(read(&skills_dir.join("beta/SKILL.md")), "new beta");

        cleanup_staged_skills(tmp.path(), Harness::ClaudeCode).unwrap();

        assert_eq!(read(&skills_dir.join("alpha/SKILL.md")), "USER ALPHA");
        assert!(!skills_dir.join("beta").exists());
        assert!(!skills_dir.join(STAGED_SIBLING_MANIFEST).exists());
    }

    #[test]
    fn sweeps_prefix_entries_when_no_manifest() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join(".claude/skills");
        fs::create_dir_all(skills_dir.join("slow-powers-eval-1-with_skill__foo")).unwrap();
        fs::create_dir_all(skills_dir.join("user-custom")).unwrap();

        cleanup_staged_skills(tmp.path(), Harness::ClaudeCode).unwrap();

        assert!(
            !skills_dir
                .join("slow-powers-eval-1-with_skill__foo")
                .exists()
        );
        assert!(skills_dir.join("user-custom").exists());
    }

    // ── cleanup_staged_skills (runner-created .claude/skills) ──────────────

    #[test]
    fn removes_whole_tree_when_runner_created_and_prunes_claude() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src-skills");
        write(&src.join("alpha/SKILL.md"), "alpha");

        stage_sibling_skills(&StageSiblingOpts {
            skill_under_test: "x",
            skills_source_dir: &src,
            repo_root: tmp.path(),
            ..Default::default()
        })
        .unwrap();
        fs::create_dir_all(tmp.path().join(".claude/skills/stray-leftover")).unwrap();

        cleanup_staged_skills(tmp.path(), Harness::ClaudeCode).unwrap();

        assert!(!tmp.path().join(".claude/skills").exists());
        assert!(!tmp.path().join(".claude").exists());
    }

    #[test]
    fn keeps_claude_and_settings_when_runner_created_only_skills() {
        let tmp = TempDir::new().unwrap();
        let claude_dir = tmp.path().join(".claude");
        write(&claude_dir.join("settings.json"), "{}");
        let src = tmp.path().join("src-skills");
        write(&src.join("alpha/SKILL.md"), "alpha");

        stage_sibling_skills(&StageSiblingOpts {
            skill_under_test: "x",
            skills_source_dir: &src,
            repo_root: tmp.path(),
            ..Default::default()
        })
        .unwrap();

        cleanup_staged_skills(tmp.path(), Harness::ClaudeCode).unwrap();

        assert!(!claude_dir.join("skills").exists());
        assert!(claude_dir.exists());
        assert!(claude_dir.join("settings.json").exists());
    }

    #[test]
    fn leaves_preexisting_skills_dir_in_place() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join(".claude/skills");
        write(&skills_dir.join("user-owned/SKILL.md"), "USER");
        let src = tmp.path().join("src-skills");
        write(&src.join("alpha/SKILL.md"), "alpha");

        stage_sibling_skills(&StageSiblingOpts {
            skill_under_test: "x",
            skills_source_dir: &src,
            repo_root: tmp.path(),
            ..Default::default()
        })
        .unwrap();

        cleanup_staged_skills(tmp.path(), Harness::ClaudeCode).unwrap();

        assert!(skills_dir.exists());
        assert_eq!(read(&skills_dir.join("user-owned/SKILL.md")), "USER");
        assert!(!skills_dir.join("alpha").exists());
    }
}
