//! `RunContext` detection.
//!
//! `clap` owns flag parsing, so `detect_run_context` takes already-parsed
//! values (a [`DetectInput`]) and performs the filesystem validation,
//! sibling-skill enumeration, and path defaulting that produce a
//! [`RunContext`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::run_mode::{RunMode, resolve_run_mode};

/// The agent harness an eval runs against. Single source of truth, shared with
/// the CLI layer (it derives `clap::ValueEnum` so flags can parse it directly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum Harness {
    #[default]
    ClaudeCode,
    Codex,
    #[serde(rename = "opencode")]
    #[value(name = "opencode")]
    OpenCode,
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
    pub stage_siblings: bool,
    pub workspace_root: PathBuf,
    pub stage_root: PathBuf,
    pub bootstrap_path: Option<PathBuf>,
    pub harness: Harness,
    /// The resolved run mode (the dispatch mechanism + who drives the loop).
    /// Resolved per harness from the `--run-mode` flag in [`detect_run_context`].
    pub run_mode: RunMode,
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
    pub run_mode: Option<RunMode>,
    pub cwd: Option<PathBuf>,
}

/// A user-facing failure while detecting the run context. Display strings carry
/// the offending flag/path so the `error: <msg>` boundary in `main.rs` is
/// actionable.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error(
        "missing skill. Run from a skill directory containing SKILL.md, pass --skill <path-or-name>, or pass --skill-dir <dir> --skill <name>"
    )]
    MissingSkill,
    #[error("--skill-dir contains multiple skills; pass --skill <name>. Candidates: {0}")]
    AmbiguousSkillSelection(String),
    #[error("no skills found under --skill-dir: {0}")]
    NoSkillsInSkillDir(String),
    #[error("--skill-dir is not a directory: {0}")]
    SkillDirNotDirectory(String),
    #[error("skill not found: {0}")]
    SkillNotFound(String),
    #[error("--bootstrap file not found: {0}")]
    BootstrapNotFound(String),
    #[error("{0}")]
    UnsupportedRunMode(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Lexically absolutize a path (join onto cwd if relative; normalize `.`/`..`).
/// Mirrors node's `resolve()` — it does NOT resolve symlinks or require
/// existence, unlike `std::fs::canonicalize`.
fn absolutize(cwd: &Path, p: &str) -> Result<PathBuf, ContextError> {
    let path = Path::new(p);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    Ok(std::path::absolute(joined)?)
}

fn skill_name_from_dir(skill_subdir: &Path) -> Result<String, ContextError> {
    skill_subdir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .ok_or(ContextError::MissingSkill)
}

fn parent_dir(skill_subdir: &Path) -> PathBuf {
    skill_subdir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| skill_subdir.to_path_buf())
}

fn enumerate_skill_children(skill_dir: &Path) -> Result<Vec<String>, ContextError> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(skill_dir)? {
        let entry = entry?;
        let sub = entry.path();
        if !sub.is_dir() || !sub.join("SKILL.md").exists() {
            continue;
        }
        out.push(entry.file_name().to_string_lossy().into_owned());
    }
    out.sort();
    Ok(out)
}

/// Other dirs in `skill_dir` (excluding the skill-under-test) that contain a
/// `SKILL.md`. Sorted for deterministic output.
fn enumerate_siblings(skill_dir: &Path, skill_name: &str) -> Result<Vec<String>, ContextError> {
    Ok(enumerate_skill_children(skill_dir)?
        .into_iter()
        .filter(|name| name != skill_name)
        .collect())
}

fn infer_only_skill_name(skill_dir: &Path) -> Result<String, ContextError> {
    let skills = enumerate_skill_children(skill_dir)?;
    match skills.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(ContextError::NoSkillsInSkillDir(
            skill_dir.display().to_string(),
        )),
        _ => Err(ContextError::AmbiguousSkillSelection(skills.join(", "))),
    }
}

/// Validate the parsed flags against the filesystem and assemble a
/// [`RunContext`]: resolves either a seeded `--skill-dir` environment or a direct
/// single skill selected from `--skill <path-or-name>` / the current directory,
/// validates `SKILL.md`, an optional existing `--bootstrap`, and defaults the
/// workspace/stage roots from the current directory.
pub fn detect_run_context(input: DetectInput) -> Result<RunContext, ContextError> {
    let cwd = input.cwd.unwrap_or(std::env::current_dir()?);
    let cwd = std::path::absolute(cwd)?;
    let (skill_dir, skill_name, skill_subdir, sibling_skill_names, stage_siblings) =
        match input.skill_dir {
            Some(skill_dir_raw) => {
                let skill_dir = absolutize(&cwd, &skill_dir_raw)?;
                if !skill_dir.is_dir() {
                    return Err(ContextError::SkillDirNotDirectory(
                        skill_dir.display().to_string(),
                    ));
                }
                let skill_name = match input.skill {
                    Some(skill) => skill,
                    None => infer_only_skill_name(&skill_dir)?,
                };
                let skill_subdir = skill_dir.join(&skill_name);
                let sibling_skill_names = enumerate_siblings(&skill_dir, &skill_name)?;
                (
                    skill_dir,
                    skill_name,
                    skill_subdir,
                    sibling_skill_names,
                    true,
                )
            }
            None => {
                let skill_subdir = match input.skill {
                    Some(skill_raw) => absolutize(&cwd, &skill_raw)?,
                    None if cwd.join("SKILL.md").exists() => cwd.clone(),
                    None => return Err(ContextError::MissingSkill),
                };
                let skill_name = skill_name_from_dir(&skill_subdir)?;
                let skill_dir = parent_dir(&skill_subdir);
                (skill_dir, skill_name, skill_subdir, Vec::new(), false)
            }
        };
    let skill_md = skill_subdir.join("SKILL.md");
    if !skill_md.exists() {
        return Err(ContextError::SkillNotFound(skill_md.display().to_string()));
    }

    let bootstrap_path = match input.bootstrap {
        Some(raw) => {
            let resolved = absolutize(&cwd, &raw)?;
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
        Some(raw) => absolutize(&cwd, &raw)?,
        None => cwd.join(".eval-magic"),
    };
    let stage_root = cwd;

    let harness = input.harness.unwrap_or_default();
    let run_mode =
        resolve_run_mode(harness, input.run_mode).map_err(ContextError::UnsupportedRunMode)?;

    Ok(RunContext {
        skill_dir,
        skill_name,
        skill_subdir,
        sibling_skill_names,
        stage_siblings,
        workspace_root,
        stage_root,
        bootstrap_path,
        harness,
        run_mode,
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

    fn input_from(cwd: &Path) -> DetectInput {
        DetectInput {
            cwd: Some(cwd.to_path_buf()),
            ..Default::default()
        }
    }

    #[test]
    fn cwd_skill_dir_is_the_default_single_skill() {
        let tmp = TempDir::new().unwrap();
        let skill_subdir = tmp.path().join("mr-review");
        fs::create_dir_all(&skill_subdir).unwrap();
        fs::write(
            skill_subdir.join("SKILL.md"),
            "---\nname: mr-review\n---\n\nbody\n",
        )
        .unwrap();

        let ctx = detect_run_context(input_from(&skill_subdir)).unwrap();

        assert_eq!(ctx.skill_name, "mr-review");
        assert_eq!(
            ctx.skill_subdir,
            std::path::absolute(&skill_subdir).unwrap()
        );
        assert!(ctx.sibling_skill_names.is_empty());
        assert!(!ctx.stage_siblings);
    }

    #[test]
    fn skill_path_selects_one_skill_without_siblings() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["alpha", "beta"]);

        let ctx = detect_run_context(DetectInput {
            skill: Some(skill_dir.join("beta").to_string_lossy().into_owned()),
            cwd: Some(tmp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(ctx.skill_name, "beta");
        assert_eq!(
            ctx.skill_subdir,
            std::path::absolute(skill_dir.join("beta")).unwrap()
        );
        assert!(ctx.sibling_skill_names.is_empty());
        assert!(!ctx.stage_siblings);
    }

    #[test]
    fn skill_dir_with_one_skill_infers_the_skill_name_and_stages_siblings_mode() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["only-skill"]);

        let ctx = detect_run_context(DetectInput {
            skill_dir: Some(skill_dir.to_string_lossy().into_owned()),
            cwd: Some(tmp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(ctx.skill_name, "only-skill");
        assert!(ctx.sibling_skill_names.is_empty());
        assert!(ctx.stage_siblings);
    }

    #[test]
    fn skill_dir_with_multiple_skills_requires_a_skill_name() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["alpha", "beta"]);

        let err = detect_run_context(DetectInput {
            skill_dir: Some(skill_dir.to_string_lossy().into_owned()),
            cwd: Some(tmp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap_err();

        assert!(matches!(err, ContextError::AmbiguousSkillSelection(_)));
        assert!(err.to_string().contains("alpha"));
        assert!(err.to_string().contains("beta"));
    }

    #[test]
    fn missing_skill_errors_when_cwd_is_not_a_skill() {
        let tmp = TempDir::new().unwrap();
        let err = detect_run_context(input_from(tmp.path())).unwrap_err();
        assert!(matches!(err, ContextError::MissingSkill));
        assert!(err.to_string().contains("--skill"));
    }

    #[test]
    fn empty_skill_dir_errors_when_skill_is_not_named() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill-dir");
        fs::create_dir_all(&skill_dir).unwrap();
        let err = detect_run_context(DetectInput {
            skill_dir: Some(skill_dir.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, ContextError::NoSkillsInSkillDir(_)));
        assert!(err.to_string().contains("no skills found"));
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
        let expected = std::env::current_dir().unwrap().join(".eval-magic");
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

    #[test]
    fn harness_opencode_accepted() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let ctx = detect_run_context(DetectInput {
            harness: Some(Harness::OpenCode),
            ..input(&skill_dir, "foo")
        })
        .unwrap();
        assert_eq!(ctx.harness, Harness::OpenCode);
    }
}
