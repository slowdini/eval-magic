//! Resolve and apply eval-authored files on top of a staged codebase.

use std::fs;
use std::path::Path;

use crate::core::fs::copy_entry_materialized;
use crate::core::{AssertionCommandCheck, Eval};

use super::RunError;

/// True for a path that is absolute under either supported portable-data shape.
fn is_absolute_on_any_platform(raw: &str) -> bool {
    let path = Path::new(raw);
    path.has_root()
        || raw.starts_with('\\')
        || matches!(
            path.components().next(),
            Some(std::path::Component::Prefix(_))
        )
}

/// Reject a task-relative path that can escape the private environment or
/// replace runner-owned root Git metadata.
fn validate_task_relative_path(raw: &str) -> Result<(), RunError> {
    let path = Path::new(raw);
    let escapes = is_absolute_on_any_platform(raw)
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir));
    if escapes {
        return Err(RunError::msg(format!(
            "task-relative path must be relative and stay within the environment: {raw}"
        )));
    }
    let first_normal = path.components().find_map(|component| match component {
        std::path::Component::Normal(value) => Some(value.to_string_lossy()),
        _ => None,
    });
    if first_normal.is_some_and(|component| component.eq_ignore_ascii_case(".git")) {
        return Err(RunError::msg(format!(
            "task-relative path uses reserved runner-owned Git metadata at task root: {raw}"
        )));
    }
    Ok(())
}

/// Reject an overlay source root that escapes `<skill>/evals/`.
fn validate_files_root_rel(root: &str) -> Result<(), RunError> {
    let path = Path::new(root);
    let escapes = is_absolute_on_any_platform(root)
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir));
    if escapes {
        return Err(RunError::msg(format!(
            "files_root must be relative and stay within the skill's evals directory: {root}"
        )));
    }
    Ok(())
}

/// Resolve an eval's overlay files without copying them.
pub fn overlay_file_pairs(
    eval: &Eval,
    skill_dir: &Path,
) -> Result<Vec<(String, String)>, RunError> {
    let mut source_root = skill_dir.join("evals");
    if let Some(root) = eval.files_root.as_deref() {
        validate_files_root_rel(root)?;
        source_root = source_root.join(root);
    }
    let Some(files) = eval.files.as_ref().filter(|files| !files.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut pairs = Vec::with_capacity(files.len());
    for file in files {
        validate_task_relative_path(file)?;
        let source = source_root.join(file);
        if !source.exists() {
            return Err(RunError::msg(format!(
                "overlay file not found: {}",
                source.display()
            )));
        }
        pairs.push((file.clone(), source.to_string_lossy().into_owned()));
    }
    Ok(pairs)
}

/// Resolve a command check's held-out setup paths without copying them.
pub fn setup_file_pairs(
    check: &AssertionCommandCheck,
    skill_dir: &Path,
) -> Result<Vec<(String, String)>, RunError> {
    let Some(files) = check.setup_files.as_ref().filter(|files| !files.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut pairs = Vec::with_capacity(files.len());
    for file in files {
        validate_task_relative_path(file)?;
        let source = skill_dir.join("evals").join(file);
        if !source.exists() {
            return Err(RunError::msg(format!(
                "command-check setup file not found: {}",
                source.display()
            )));
        }
        pairs.push((file.clone(), source.to_string_lossy().into_owned()));
    }
    Ok(pairs)
}

/// Copy an eval's overlay files into its private task environment.
pub fn copy_overlay_files(
    eval: &Eval,
    skill_dir: &Path,
    env_root: &Path,
) -> Result<Vec<String>, RunError> {
    let pairs = overlay_file_pairs(eval, skill_dir)?;
    let mut copied = Vec::with_capacity(pairs.len());
    for (dest, source) in &pairs {
        let destination = env_root.join(dest);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        copy_entry_materialized(Path::new(source), &destination)?;
        copied.push(dest.clone());
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_with_files(files: &[&str]) -> Eval {
        Eval {
            id: "e1".to_string(),
            prompt: "p".to_string(),
            expected_output: "o".to_string(),
            files: Some(files.iter().map(|file| (*file).to_string()).collect()),
            files_root: None,
            assertions: None,
            skill_should_trigger: None,
            runs: None,
            turns: None,
            codebase: None,
            responder: None,
            guard: None,
            plan_mode: false,
        }
    }

    #[test]
    fn overlay_pairs_resolve_without_copying() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill");
        let evals = skill_dir.join("evals");
        fs::create_dir_all(evals.join("data")).unwrap();
        fs::write(evals.join("config.json"), "cfg").unwrap();
        fs::write(evals.join("data/x.json"), "xx").unwrap();

        let pairs = overlay_file_pairs(
            &eval_with_files(&["config.json", "data/x.json"]),
            &skill_dir,
        )
        .unwrap();

        assert_eq!(
            pairs,
            vec![
                (
                    "config.json".to_string(),
                    evals.join("config.json").to_string_lossy().into_owned()
                ),
                (
                    "data/x.json".to_string(),
                    evals.join("data/x.json").to_string_lossy().into_owned()
                ),
            ]
        );
        assert!(!tmp.path().join("env").exists());
    }

    #[test]
    fn overlay_pairs_validate_source_root_and_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill");
        fs::create_dir_all(skill_dir.join("evals")).unwrap();

        let mut escaping_root = eval_with_files(&[]);
        escaping_root.files_root = Some("../outside".to_string());
        let error = overlay_file_pairs(&escaping_root, &skill_dir)
            .unwrap_err()
            .to_string();
        assert!(error.contains("files_root"), "error was: {error}");

        let missing = overlay_file_pairs(&eval_with_files(&["missing.txt"]), &skill_dir)
            .unwrap_err()
            .to_string();
        assert!(missing.contains("overlay file not found"), "{missing}");
    }

    #[test]
    fn task_paths_reject_escapes_and_root_git_metadata() {
        for bad in [
            "../escape.txt",
            "/etc/passwd",
            "a/../../b.txt",
            r"\etc\passwd",
        ] {
            let error = validate_task_relative_path(bad).unwrap_err().to_string();
            assert!(error.contains("relative"), "{bad}: {error}");
        }
        for bad in [".git", ".git/config", "./.GIT/config"] {
            let error = validate_task_relative_path(bad).unwrap_err().to_string();
            assert!(error.contains("reserved"), "{bad}: {error}");
        }
        validate_task_relative_path("vendor/.git/config").unwrap();
    }

    #[test]
    fn copy_overlay_files_preserves_task_relative_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill");
        let evals = skill_dir.join("evals");
        fs::create_dir_all(evals.join("data")).unwrap();
        fs::write(evals.join("config.json"), "cfg").unwrap();
        fs::write(evals.join("data/x.json"), "xx").unwrap();
        let env_root = tmp.path().join("env");

        let copied = copy_overlay_files(
            &eval_with_files(&["config.json", "data/x.json"]),
            &skill_dir,
            &env_root,
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(env_root.join("config.json")).unwrap(),
            "cfg"
        );
        assert_eq!(
            fs::read_to_string(env_root.join("data/x.json")).unwrap(),
            "xx"
        );
        assert_eq!(copied, vec!["config.json", "data/x.json"]);
    }
}
