//! `RunContext` detection.
//!
//! `clap` owns flag parsing, so `detect_run_context` takes already-parsed
//! values (a [`DetectInput`]) and performs the filesystem validation,
//! sibling-skill enumeration, and path defaulting that produce a
//! [`RunContext`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The agent harness an eval runs against. Single source of truth, shared with
/// the CLI layer (it derives `clap::ValueEnum` so flags can parse it directly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum Harness {
    #[default]
    ClaudeCode,
    Codex,
}

/// The resolved environment for a run: validated skill location, sibling skills,
/// workspace/stage roots, optional bootstrap file, and the target harness. Built
/// by [`detect_run_context`]; held in memory and never (de)serialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunContext {
    pub skill_dir: PathBuf,
    pub skill_name: String,
    pub skill_subdir: PathBuf,
    pub sibling_skill_names: Vec<String>,
    pub workspace_root: PathBuf,
    pub stage_root: PathBuf,
    pub bootstrap_path: Option<PathBuf>,
    pub harness: Harness,
}

/// Already-parsed flag values handed to [`detect_run_context`]. `clap` owns the
/// actual argv parsing (and, once wired, the harness `ValueEnum` rejection); this
/// struct carries the raw values through to filesystem validation and defaulting.
#[derive(Debug, Clone, Default)]
pub struct DetectInput {
    pub skill_dir: Option<String>,
    pub skill: Option<String>,
    pub bootstrap: Option<String>,
    pub workspace_dir: Option<String>,
    pub harness: Option<Harness>,
}

/// A user-facing failure while detecting the run context. Display strings carry
/// the offending flag/path so the `error: <msg>` boundary in `main.rs` is
/// actionable.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("missing required flag --skill-dir <path>")]
    MissingSkillDir,
    #[error("missing required flag --skill <name>")]
    MissingSkill,
    #[error("--skill-dir is not a directory: {0}")]
    SkillDirNotDirectory(String),
    #[error("skill not found: {0}")]
    SkillNotFound(String),
    #[error("--bootstrap file not found: {0}")]
    BootstrapNotFound(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Lexically absolutize a path (join onto cwd if relative; normalize `.`/`..`).
/// Mirrors node's `resolve()` — it does NOT resolve symlinks or require
/// existence, unlike `std::fs::canonicalize`.
fn absolutize(p: &str) -> Result<PathBuf, ContextError> {
    Ok(std::path::absolute(p)?)
}

/// Other dirs in `skill_dir` (excluding the skill-under-test) that contain a
/// `SKILL.md`. Sorted for deterministic output.
fn enumerate_siblings(skill_dir: &Path, skill_name: &str) -> Result<Vec<String>, ContextError> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(skill_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == skill_name {
            continue;
        }
        let sub = entry.path();
        if !sub.is_dir() {
            continue;
        }
        if !sub.join("SKILL.md").exists() {
            continue;
        }
        out.push(name);
    }
    out.sort();
    Ok(out)
}

/// Validate the parsed flags against the filesystem and assemble a
/// [`RunContext`]: requires an existing `--skill-dir`, a `--skill` whose
/// `SKILL.md` exists, an optional existing `--bootstrap`, and defaults the
/// workspace/stage roots from the current directory.
pub fn detect_run_context(input: DetectInput) -> Result<RunContext, ContextError> {
    let skill_dir_raw = input.skill_dir.ok_or(ContextError::MissingSkillDir)?;
    let skill_dir = absolutize(&skill_dir_raw)?;
    if !skill_dir.is_dir() {
        return Err(ContextError::SkillDirNotDirectory(
            skill_dir.display().to_string(),
        ));
    }

    let skill_name = input.skill.ok_or(ContextError::MissingSkill)?;
    let skill_subdir = skill_dir.join(&skill_name);
    let skill_md = skill_subdir.join("SKILL.md");
    if !skill_md.exists() {
        return Err(ContextError::SkillNotFound(skill_md.display().to_string()));
    }

    let bootstrap_path = match input.bootstrap {
        Some(raw) => {
            let resolved = absolutize(&raw)?;
            if !resolved.exists() {
                return Err(ContextError::BootstrapNotFound(
                    resolved.display().to_string(),
                ));
            }
            Some(resolved)
        }
        None => None,
    };

    let workspace_root = match input.workspace_dir {
        Some(raw) => absolutize(&raw)?,
        None => std::env::current_dir()?.join("skills-workspace"),
    };
    let stage_root = std::env::current_dir()?;

    let harness = input.harness.unwrap_or_default();
    let sibling_skill_names = enumerate_siblings(&skill_dir, &skill_name)?;

    Ok(RunContext {
        skill_dir,
        skill_name,
        skill_subdir,
        sibling_skill_names,
        workspace_root,
        stage_root,
        bootstrap_path,
        harness,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    /// Build `<root>/skill-dir` containing one subdir per name, each with a
    /// `SKILL.md`, and return the skill-dir path.
    fn make_skill_dir(root: &Path, skills: &[&str]) -> PathBuf {
        let dir = root.join("skill-dir");
        fs::create_dir_all(&dir).unwrap();
        for name in skills {
            let sub = dir.join(name);
            fs::create_dir_all(&sub).unwrap();
            fs::write(
                sub.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {name} skill\n---\n\nbody\n"),
            )
            .unwrap();
        }
        dir
    }

    fn input(skill_dir: &Path, skill: &str) -> DetectInput {
        DetectInput {
            skill_dir: Some(skill_dir.to_string_lossy().into_owned()),
            skill: Some(skill.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn missing_skill_dir_errors() {
        let err = detect_run_context(DetectInput {
            skill: Some("foo".into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, ContextError::MissingSkillDir));
        assert!(err.to_string().contains("--skill-dir"));
    }

    #[test]
    fn missing_skill_errors() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let err = detect_run_context(DetectInput {
            skill_dir: Some(skill_dir.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, ContextError::MissingSkill));
        assert!(err.to_string().contains("--skill"));
    }

    #[test]
    fn skill_dir_not_directory_errors() {
        let err = detect_run_context(DetectInput {
            skill_dir: Some("/nonexistent/does-not-exist-12345".into()),
            skill: Some("foo".into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, ContextError::SkillDirNotDirectory(_)));
        assert!(err.to_string().contains("--skill-dir"));
    }

    #[test]
    fn skill_subdir_missing_errors() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let err = detect_run_context(input(&skill_dir, "bar")).unwrap_err();
        assert!(matches!(err, ContextError::SkillNotFound(_)));
        assert!(err.to_string().contains("skill not found"));
    }

    #[test]
    fn bad_bootstrap_errors() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let err = detect_run_context(DetectInput {
            bootstrap: Some("/nonexistent/no-bootstrap-12345.md".into()),
            ..input(&skill_dir, "foo")
        })
        .unwrap_err();
        assert!(matches!(err, ContextError::BootstrapNotFound(_)));
        assert!(err.to_string().contains("--bootstrap"));
    }

    #[test]
    fn happy_path_absolute_paths() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["mr-review"]);
        let ctx = detect_run_context(input(&skill_dir, "mr-review")).unwrap();
        assert_eq!(ctx.skill_dir, std::path::absolute(&skill_dir).unwrap());
        assert_eq!(ctx.skill_name, "mr-review");
        assert_eq!(
            ctx.skill_subdir,
            std::path::absolute(skill_dir.join("mr-review")).unwrap()
        );
        assert!(ctx.sibling_skill_names.is_empty());
        assert!(ctx.bootstrap_path.is_none());
        assert_eq!(ctx.harness, Harness::ClaudeCode);
    }

    #[test]
    fn enumerates_siblings_excluding_sut() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["alpha", "beta", "gamma"]);
        let ctx = detect_run_context(input(&skill_dir, "beta")).unwrap();
        assert_eq!(
            ctx.sibling_skill_names,
            vec!["alpha".to_string(), "gamma".to_string()]
        );
    }

    #[test]
    fn ignores_non_skill_md_entries() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["real"]);
        fs::create_dir_all(skill_dir.join("node_modules")).unwrap();
        fs::create_dir_all(skill_dir.join("no-skill-md-here")).unwrap();
        fs::write(skill_dir.join("loose-file.txt"), "hello").unwrap();
        let ctx = detect_run_context(input(&skill_dir, "real")).unwrap();
        assert!(ctx.sibling_skill_names.is_empty());
    }

    #[test]
    fn workspace_default() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let ctx = detect_run_context(input(&skill_dir, "foo")).unwrap();
        let expected = std::env::current_dir().unwrap().join("skills-workspace");
        assert_eq!(ctx.workspace_root, expected);
    }

    #[test]
    fn workspace_override_absolute() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let custom = tmp.path().join("custom-ws");
        fs::create_dir_all(&custom).unwrap();
        let ctx = detect_run_context(DetectInput {
            workspace_dir: Some(custom.to_string_lossy().into_owned()),
            ..input(&skill_dir, "foo")
        })
        .unwrap();
        assert_eq!(ctx.workspace_root, std::path::absolute(&custom).unwrap());
    }

    #[test]
    fn stage_root_default() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let ctx = detect_run_context(input(&skill_dir, "foo")).unwrap();
        assert_eq!(ctx.stage_root, std::env::current_dir().unwrap());
    }

    #[test]
    fn bootstrap_resolved_absolute() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let bootstrap = tmp.path().join("my-bootstrap.md");
        fs::write(&bootstrap, "BOOT").unwrap();
        let ctx = detect_run_context(DetectInput {
            bootstrap: Some(bootstrap.to_string_lossy().into_owned()),
            ..input(&skill_dir, "foo")
        })
        .unwrap();
        assert_eq!(
            ctx.bootstrap_path,
            Some(std::path::absolute(&bootstrap).unwrap())
        );
    }

    #[test]
    fn harness_codex_accepted() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let ctx = detect_run_context(DetectInput {
            harness: Some(Harness::Codex),
            ..input(&skill_dir, "foo")
        })
        .unwrap();
        assert_eq!(ctx.harness, Harness::Codex);
    }
}
