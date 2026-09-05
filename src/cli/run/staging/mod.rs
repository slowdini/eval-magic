//! Staged-skill lifecycle: install a skill (and its siblings) into the harness's
//! project-local skills dir so eval subagents can discover it, and tear that
//! staging back down — restoring any pre-existing skills the runner displaced.
//!
//! The sibling-staging manifest
//! (`.slow-powers-eval-manifest.json`) records what the runner created and what
//! it backed up, so [`cleanup_staged_skills`] can surgically undo only its own
//! changes and leave the user's own project skills intact.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::adapters::{adapter_for, all_config_dir_names};
use crate::core::Harness;
use crate::pipeline::io::now_iso8601;
use crate::workspace::SNAPSHOT_META;

use super::RunError;
use crate::core::fs::{copy_entry_materialized, write_json};

mod codebase;
pub use codebase::exclude_codebase_skill_sources;
use codebase::{ExcludedRoot, is_managed_backup_path};

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_roots: Vec<ExcludedRoot>,
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
            harness: Harness::default(),
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
            harness: Harness::default(),
        }
    }
}

/// `<repo_root>/.agents/skills` (Codex) or `<repo_root>/.claude/skills`.
pub(crate) fn skills_dir_for_harness(repo_root: &Path, harness: Harness) -> PathBuf {
    adapter_for(harness)
        .skills_dir(repo_root)
        .expect("staging requires skills_dir; the run preflight forces --no-stage otherwise")
}

/// True when `name` is any harness's project-local config dir (`.claude`,
/// `.agents`, …). Staging excludes every harness's config dirs when copying a
/// skill's sibling assets — regardless of the active harness — so a checked-in
/// config dir never rides into a staged env.
fn is_harness_config_dir(name: &str) -> bool {
    all_config_dir_names().iter().any(|d| d == name)
}

/// Rewrite (or insert) the `name:` frontmatter field so a Codex-staged skill's
/// declared name matches its staged slug.
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

/// Stage one skill under the harness's skills dir and return its slug. For
/// harnesses whose adapter opts in, the frontmatter `name:` is rewritten to the
/// slug.
pub fn stage_skill_for_harness(opts: &StageSkillOpts) -> Result<String, RunError> {
    let adapter = adapter_for(opts.harness);
    let slug = match opts.stage_name_override {
        Some(name) => name.to_string(),
        None => adapter.staged_slug(
            STAGED_SKILL_PREFIX,
            opts.iteration,
            opts.condition,
            opts.skill_name,
        ),
    };
    adapter.validate_stage_name(&slug).map_err(RunError::msg)?;
    let skills_dir = skills_dir_for_harness(opts.repo_root, opts.harness);
    let skill_dir = skills_dir.join(&slug);
    if opts.stage_name_override.is_some() && skill_dir.exists() {
        return Err(RunError::msg(format!(
            "--stage-name \"{slug}\": {} already exists; refusing to clobber it. Remove it or choose a different name.",
            skill_dir.display()
        )));
    }
    let mut manifest = load_or_create_manifest(&skills_dir, opts.skill_name)?;
    prepare_created_entry(&skills_dir, &slug, &mut manifest)?;
    fs::create_dir_all(&skill_dir)?;

    let content = if adapter.rewrites_frontmatter_name() {
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
            let name_str = name.to_string_lossy();
            if matches!(
                name_str.as_ref(),
                "SKILL.md" | "evals" | SNAPSHOT_META | ".eval-magic"
            ) || is_harness_config_dir(name_str.as_ref())
            {
                continue;
            }
            copy_entry_materialized(&assets_dir.join(&name), &skill_dir.join(&name))?;
        }
    }
    Ok(slug)
}

/// Stage a skill for Claude Code (`.claude/skills`). Convenience wrapper over
/// [`stage_skill_for_harness`] for the tests — the orchestrator always passes
/// an explicit harness.
#[cfg(test)]
pub fn stage_skill_for_cc(opts: &StageSkillOpts) -> Result<String, RunError> {
    stage_skill_for_harness(&StageSkillOpts {
        harness: Harness::resolve("claude-code").unwrap(),
        ..opts.clone()
    })
}

/// Record a custom-named staged dir (one created via `stage_name_override`) in
/// the sibling manifest so the next run's [`cleanup_staged_skills`] removes it —
/// the prefix scan only catches `slow-powers-eval-…`. Idempotent.
pub fn register_staged_skill_for_cleanup(
    repo_root: &Path,
    name: &str,
    harness: Harness,
) -> Result<(), RunError> {
    let skills_dir = skills_dir_for_harness(repo_root, harness);
    let manifest_path = skills_dir.join(STAGED_SIBLING_MANIFEST);
    let mut manifest = load_or_create_manifest(&skills_dir, name)?;
    if manifest.created_entries.iter().any(|e| e.name == name) {
        return Ok(());
    }
    manifest.created_entries.push(CreatedEntry {
        name: name.to_string(),
        preexisting: false,
        backup_path: None,
    });
    Ok(write_json(&manifest_path, &manifest)?)
}

/// Stage every non-test sibling skill (each `<name>/` with a `SKILL.md`, minus
/// its `evals/`) into the harness skills dir, backing up any colliding
/// pre-existing entry, and write the manifest.
pub fn stage_sibling_skills(opts: &StageSiblingOpts) -> Result<SiblingManifest, RunError> {
    stage_sibling_skills_excluding(opts, &[opts.skill_under_test.to_string()])
}

/// Stage ambient skills while excluding a complete coordinated treatment set.
/// The scalar wrapper above preserves the established public helper contract.
pub fn stage_sibling_skills_excluding(
    opts: &StageSiblingOpts,
    skills_under_test: &[String],
) -> Result<SiblingManifest, RunError> {
    let skills_dir = skills_dir_for_harness(opts.repo_root, opts.harness);
    let staged_label = skills_under_test.join(",");
    let mut manifest = load_or_create_manifest(&skills_dir, &staged_label)?;
    fs::create_dir_all(&skills_dir)?;
    write_json(&skills_dir.join(STAGED_SIBLING_MANIFEST), &manifest)?;

    let mut siblings: Vec<String> = Vec::new();
    for entry in fs::read_dir(opts.skills_source_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if skills_under_test.contains(&name) {
            continue;
        }
        let src_dir = opts.skills_source_dir.join(&name);
        if !src_dir.is_dir() || !src_dir.join("SKILL.md").exists() {
            continue;
        }
        siblings.push(name);
    }
    siblings.sort();

    for name in siblings {
        let src_dir = opts.skills_source_dir.join(&name);
        let dst_dir = skills_dir.join(&name);
        prepare_created_entry(&skills_dir, &name, &mut manifest)?;

        // Copy the source skill minus its `evals/` subdir.
        fs::create_dir_all(&dst_dir)?;
        for child in fs::read_dir(&src_dir)? {
            let child = child?;
            if child.file_name() == "evals" {
                continue;
            }
            copy_entry_materialized(&child.path(), &dst_dir.join(child.file_name()))?;
        }
    }

    write_json(&skills_dir.join(STAGED_SIBLING_MANIFEST), &manifest)?;
    Ok(manifest)
}

/// Load this run's staging manifest, or initialize one before the runner creates
/// the skills directory. Capturing preexistence here lets cleanup distinguish a
/// user/codebase-owned root from one created solely for staging.
fn load_or_create_manifest(
    skills_dir: &Path,
    staged_under_test: &str,
) -> Result<SiblingManifest, RunError> {
    let manifest_path = skills_dir.join(STAGED_SIBLING_MANIFEST);
    if manifest_path.exists() {
        return Ok(serde_json::from_str(&fs::read_to_string(manifest_path)?)?);
    }
    Ok(SiblingManifest {
        created_at: now_iso8601(),
        staged_under_test: staged_under_test.to_string(),
        skills_dir_preexisting: Some(skills_dir.exists()),
        created_entries: Vec::new(),
        excluded_roots: Vec::new(),
    })
}

/// Register one runner-owned destination and preserve the entry it displaced.
/// Re-staging the same runner entry replaces only the staged copy and retains
/// the original first backup.
fn prepare_created_entry(
    skills_dir: &Path,
    name: &str,
    manifest: &mut SiblingManifest,
) -> Result<(), RunError> {
    let target = skills_dir.join(name);
    if manifest
        .created_entries
        .iter()
        .any(|entry| entry.name == name)
    {
        if target.exists() {
            remove_path(&target)?;
        }
        return Ok(());
    }

    let mut entry = CreatedEntry {
        name: name.to_string(),
        preexisting: target.exists(),
        backup_path: None,
    };
    if target.exists() {
        let backup_root = make_backup_root()?;
        let backup_path = backup_root.join(name);
        copy_entry_materialized(&target, &backup_path)?;
        entry.backup_path = Some(backup_path.display().to_string());
    }
    manifest.created_entries.push(entry);
    fs::create_dir_all(skills_dir)?;
    write_json(&skills_dir.join(STAGED_SIBLING_MANIFEST), manifest)?;
    if target.exists() {
        remove_path(&target)?;
    }
    Ok(())
}

/// Remove the staged skills (prefix-scanned + manifest-listed) and restore any
/// pre-existing siblings the runner displaced.
pub fn cleanup_staged_skills(repo_root: &Path, harness: Harness) -> Result<(), RunError> {
    let skills_dir = skills_dir_for_harness(repo_root, harness);
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

    if !manifest.excluded_roots.is_empty() {
        let allowed_roots = adapter_for(harness).project_skill_dirs(repo_root);
        for excluded in &manifest.excluded_roots {
            let target = repo_root.join(&excluded.path);
            if !allowed_roots.contains(&target) {
                return Err(RunError::msg(format!(
                    "staging manifest names undeclared project skill root {}",
                    target.display()
                )));
            }
            let backup = Path::new(&excluded.backup_path);
            if !is_managed_backup_path(backup, "skill-root") {
                return Err(RunError::msg(format!(
                    "staging manifest names unmanaged exclusion backup {}",
                    backup.display()
                )));
            }
        }
        fs::remove_dir_all(&skills_dir)?;
        for excluded in &manifest.excluded_roots {
            let target = repo_root.join(&excluded.path);
            let backup = Path::new(&excluded.backup_path);
            if backup.exists() {
                if target.exists() {
                    remove_path(&target)?;
                }
                copy_entry_materialized(backup, &target)?;
                if let Some(parent) = backup.parent() {
                    fs::remove_dir_all(parent)?;
                }
            }
        }
        if !skills_dir.exists()
            && let Some(harness_dir) = skills_dir.parent()
        {
            prune_if_empty(harness_dir)?;
        }
        return Ok(());
    }

    // The runner created the harness skills dir this run, so it holds none of the
    // user's own skills — remove the whole staged tree (including any stray,
    // non-prefixed dirs left behind), then prune an emptied parent.
    if manifest.skills_dir_preexisting == Some(false) {
        fs::remove_dir_all(&skills_dir)?;
        // Prune the now-emptied harness config dir (the skills dir's parent).
        if let Some(harness_dir) = skills_dir.parent() {
            prune_if_empty(harness_dir)?;
        }
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
            copy_entry_materialized(backup, &target)?;
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
/// the name.
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
mod tests;
