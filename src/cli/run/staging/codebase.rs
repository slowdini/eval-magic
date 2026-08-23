//! Reversible removal of project skill roots from sourced task codebases.

use super::*;

/// One project skill root moved aside for a codebase that opted out of skill
/// sources. `path` is relative to the environment root and is accepted during
/// cleanup only when the resolved harness descriptor still declares it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExcludedRoot {
    pub path: String,
    pub backup_path: String,
}

/// Move every project-local skill root discoverable by `harness` out of a
/// sourced task environment. The native root is recreated only to hold the
/// staging manifest (and, when enabled, the eval skill); root instruction files
/// and all other harness config remain in place.
pub fn exclude_codebase_skill_sources(
    repo_root: &Path,
    staged_under_test: &str,
    harness: Harness,
) -> Result<(), RunError> {
    let adapter = adapter_for(harness);
    let Some(skills_dir) = adapter.skills_dir(repo_root) else {
        return Ok(());
    };
    let mut manifest = load_or_create_manifest(&skills_dir, staged_under_test)?;

    for root in adapter.project_skill_dirs(repo_root) {
        if !root.exists() {
            continue;
        }
        let relative = root.strip_prefix(repo_root).map_err(|_| {
            RunError::msg(format!(
                "project skill root {} escapes task environment {}",
                root.display(),
                repo_root.display()
            ))
        })?;
        let backup_root = make_backup_root()?;
        let backup_path = backup_root.join("skill-root");
        copy_entry_materialized(&root, &backup_path)?;
        manifest.excluded_roots.push(ExcludedRoot {
            path: relative.to_string_lossy().into_owned(),
            backup_path: backup_path.to_string_lossy().into_owned(),
        });
        remove_path(&root)?;
    }

    if !manifest.excluded_roots.is_empty() {
        fs::create_dir_all(&skills_dir)?;
        write_json(&skills_dir.join(STAGED_SIBLING_MANIFEST), &manifest)?;
    }
    Ok(())
}

pub(super) fn is_managed_backup_path(path: &Path, expected_leaf: &str) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(expected_leaf)
        && path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("slow-powers-eval-backup-"))
        && path
            .parent()
            .and_then(Path::parent)
            .is_some_and(|parent| parent == std::env::temp_dir())
}
