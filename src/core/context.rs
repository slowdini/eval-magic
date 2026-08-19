//! `RunContext` detection.
//!
//! `clap` owns flag parsing, so `detect_run_context` takes already-parsed
//! values (a [`DetectInput`]) and performs the filesystem validation,
//! sibling-skill enumeration, and path defaulting that produce a
//! [`RunContext`].

use std::path::{Path, PathBuf};

use serde::Serialize;

/// The agent harness an eval runs against: a validated handle to an entry in
/// the descriptor registry. Constructible only through registry resolution, so
/// a held `Harness` always names a registered harness and adapter lookup never
/// fails. The registry-dependent behavior lives next to the registry in
/// `crate::adapters::harness`: `Harness::resolve` (the string-to-handle
/// gateway, resolving `--harness` after parsing), `Harness::known` (every
/// registered harness), `Default` (the registry's default harness), and
/// `Deserialize` (resolves artifact values, rejecting unknown names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Harness {
    name: &'static str,
}

impl Harness {
    /// Registry-only constructor: `name` must be a registry entry's label.
    pub(crate) const fn from_static_name(name: &'static str) -> Self {
        Harness { name }
    }

    /// The kebab-case identifier (`claude-code`, `codex`, `opencode`, …) — the
    /// `--harness` flag value and the `harness` value in artifacts.
    pub fn name(self) -> &'static str {
        self.name
    }
}

impl Serialize for Harness {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name)
    }
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
    /// Things the operator should know about how this context resolved. `core`
    /// never prints; `cli::run_context_with_bootstrap` owns the `⚠ ` prefix.
    pub warnings: Vec<String>,
}

/// Already-parsed flag values handed to [`detect_run_context`]. `clap` owns the
/// actual argv parsing, and `--harness` is resolved against the registry before
/// this struct is built (unknown names are rejected there); it carries the raw
/// values through to filesystem validation and defaulting.
#[derive(Debug, Clone, Default)]
pub struct DetectInput {
    pub skill_dir: Option<String>,
    pub skill: Option<String>,
    pub bootstrap: Option<String>,
    pub workspace_dir: Option<String>,
    pub harness: Option<Harness>,
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
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Absolutize a path (join onto `cwd` if relative) and resolve it to the one
/// spelling every path in a [`RunContext`] shares — see
/// [`crate::core::fs::real_path`]. Routing every path through here is what makes
/// that a property of the struct rather than of the one field someone remembered.
fn absolutize(cwd: &Path, p: &str) -> Result<PathBuf, ContextError> {
    let path = Path::new(p);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    Ok(crate::core::fs::real_path(&joined)?)
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

/// Directory name a derived eval home is namespaced by: the skill directory's
/// own name, plus a digest of its full path.
///
/// The name alone would collide — two repositories can each hold a `code-review`
/// — and colliding roots would interleave two skills' iterations under one tree,
/// where `--iteration N` could reach the wrong one. The digest alone would be
/// unreadable. Together they are recognizable and unambiguous.
fn workspace_slug(skill_dir: &Path) -> String {
    let raw = skill_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut name: String = raw
        .chars()
        .take(32)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if name.is_empty() {
        name.push_str("skills");
    }
    format!("{name}-{}", path_digest(skill_dir))
}

/// FNV-1a over `path`, as 8 hex characters.
///
/// Hand-rolled rather than `DefaultHasher`, which carries no stability guarantee
/// across Rust releases. This digest names a directory the operator re-types and
/// that every generated command embeds; a toolchain upgrade silently relocating
/// someone's workspace is the one failure it must not have.
fn path_digest(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")[..8].to_string()
}

/// Resolve the eval home from explicit/environment inputs: `$EVAL_MAGIC_WORKSPACE_DIR`
/// as given (empty reads as unset), else `$XDG_DATA_HOME/eval-magic/<slug>`, else
/// `<home>/.local/share/eval-magic/<slug>`, else a temp-directory root.
///
/// The environment override is taken verbatim, exactly as `--workspace-dir` is:
/// someone who names a directory means that directory. Only the *derived*
/// default is namespaced, because only it has to serve every skill on the host.
pub fn workspace_root_from(
    env: Option<&str>,
    xdg_data_home: Option<&str>,
    home: Option<&Path>,
    skill_dir: &Path,
) -> PathBuf {
    if let Some(explicit) = env.filter(|value| !value.is_empty()) {
        return PathBuf::from(explicit);
    }
    let slug = workspace_slug(skill_dir);
    if let Some(xdg) = xdg_data_home.filter(|value| !value.is_empty()) {
        return Path::new(xdg).join("eval-magic").join(slug);
    }
    match home {
        Some(home) => home
            .join(".local")
            .join("share")
            .join("eval-magic")
            .join(slug),
        None => std::env::temp_dir().join("eval-magic").join(slug),
    }
}

/// [`workspace_root_from`] over the live environment.
pub fn default_workspace_root(skill_dir: &Path) -> PathBuf {
    workspace_root_from(
        std::env::var("EVAL_MAGIC_WORKSPACE_DIR").ok().as_deref(),
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::home_dir().as_deref(),
        skill_dir,
    )
}

/// The name of the pre-relocation eval home, and of the project-local descriptor
/// layer. The two are unrelated uses of one name; only the first has moved.
const LEGACY_WORKSPACE_DIR: &str = ".eval-magic";

/// Notice for an operator whose in-flight campaign lives at the old default.
///
/// Only a `<cwd>/.eval-magic` holding something other than `harnesses/` counts:
/// that subdirectory is the descriptor layer, which still belongs there.
fn legacy_workspace_notice(cwd: &Path, workspace_root: &Path) -> Option<String> {
    let legacy = cwd.join(LEGACY_WORKSPACE_DIR);
    // Nothing was left behind if the run is using that very directory.
    if workspace_root == legacy {
        return None;
    }
    let has_campaign = std::fs::read_dir(&legacy)
        .ok()?
        .filter_map(Result::ok)
        .any(|entry| entry.file_name() != std::ffi::OsStr::new("harnesses"));
    has_campaign.then(|| {
        format!(
            "a workspace from an earlier version exists at {}; artifacts now default to {}. \
             Pass --workspace-dir {} to continue the campaign already there.",
            legacy.display(),
            workspace_root.display(),
            legacy.display()
        )
    })
}

/// Validate the parsed flags against the filesystem and assemble a
/// [`RunContext`]: resolves either a seeded `--skill-dir` environment or a direct
/// single skill selected from `--skill <path-or-name>` / the current directory,
/// validates `SKILL.md`, an optional existing `--bootstrap`, and defaults the
/// workspace/stage roots from the current directory.
pub fn detect_run_context(input: DetectInput) -> Result<RunContext, ContextError> {
    let cwd = input.cwd.map_or_else(std::env::current_dir, Ok)?;
    // Every root below derives from this one path, so resolving the alias here
    // is what keeps a run's paths and an agent's paths comparable at all.
    let cwd = crate::core::fs::real_path(&cwd)?;
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

    // The eval home derives from the skill directory, not the cwd: artifacts
    // belong to the skill under test, not to wherever the operator was standing.
    let (workspace_root, warnings) = match input.workspace_dir {
        Some(raw) => (absolutize(&cwd, &raw)?, Vec::new()),
        None => {
            let root = absolutize(&cwd, &default_workspace_root(&skill_dir).to_string_lossy())?;
            let warnings = legacy_workspace_notice(&cwd, &root)
                .map(|notice| vec![notice])
                .unwrap_or_default();
            (root, warnings)
        }
    };
    let stage_root = cwd;

    let harness = input.harness.unwrap_or_default();

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
        warnings,
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
            crate::core::fs::real_path(&skill_subdir).unwrap()
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
            crate::core::fs::real_path(&skill_dir.join("beta")).unwrap()
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
        assert_eq!(
            ctx.skill_dir,
            crate::core::fs::real_path(&skill_dir).unwrap()
        );
        assert_eq!(ctx.skill_name, "mr-review");
        assert_eq!(
            ctx.skill_subdir,
            crate::core::fs::real_path(&skill_dir.join("mr-review")).unwrap()
        );
        assert!(ctx.sibling_skill_names.is_empty());
        assert!(ctx.bootstrap_path.is_none());
        assert_eq!(ctx.harness, Harness::resolve("claude-code").unwrap());
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

    /// The point of the relocation: eval artifacts stop landing inside whatever
    /// repository the operator happened to be standing in.
    #[test]
    fn workspace_default_is_outside_the_cwd_and_the_skill_tree() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let ctx = detect_run_context(input(&skill_dir, "foo")).unwrap();
        let cwd = crate::core::fs::real_path(&std::env::current_dir().unwrap()).unwrap();

        assert!(
            !ctx.workspace_root.starts_with(&cwd),
            "workspace {} is still under the cwd {}",
            ctx.workspace_root.display(),
            cwd.display()
        );
        assert!(
            !ctx.workspace_root.starts_with(&skill_dir),
            "workspace {} is still under the skill tree",
            ctx.workspace_root.display()
        );
    }

    /// `EVAL_MAGIC_WORKSPACE_DIR` sits between the flag and the derived default,
    /// mirroring the `EVAL_MAGIC_CONFIG_DIR` ladder in `descriptor::layers`.
    #[test]
    fn workspace_root_env_override_is_taken_as_given() {
        let root = workspace_root_from(
            Some("/srv/evals"),
            Some("/xdg/data"),
            Some(Path::new("/home/u")),
            Path::new("/home/u/skills"),
        );
        assert_eq!(root, PathBuf::from("/srv/evals"));
    }

    #[test]
    fn workspace_root_prefers_xdg_data_home_over_the_home_fallback() {
        let root = workspace_root_from(
            None,
            Some("/xdg/data"),
            Some(Path::new("/home/u")),
            Path::new("/home/u/skills"),
        );
        assert!(
            root.starts_with("/xdg/data/eval-magic"),
            "root was {}",
            root.display()
        );
    }

    #[test]
    fn workspace_root_falls_back_to_the_home_data_directory() {
        let root = workspace_root_from(
            None,
            None,
            Some(Path::new("/home/u")),
            Path::new("/home/u/skills"),
        );
        assert!(
            root.starts_with("/home/u/.local/share/eval-magic"),
            "root was {}",
            root.display()
        );
    }

    /// One global root would collide two skills that share a name and come from
    /// different repositories, silently interleaving their iterations. The slug
    /// is what keeps them apart.
    #[test]
    fn workspace_root_keeps_same_named_skill_dirs_apart() {
        let home = Path::new("/home/u");
        let a = workspace_root_from(None, None, Some(home), Path::new("/work/one/skills"));
        let b = workspace_root_from(None, None, Some(home), Path::new("/work/two/skills"));
        assert_ne!(a, b);
    }

    /// The slug is part of a path the operator will re-type and that generated
    /// commands embed, so it has to be the same on every run — which rules out
    /// any hash without a cross-release stability guarantee.
    #[test]
    fn workspace_root_is_stable_for_one_skill_dir() {
        let home = Path::new("/home/u");
        let skills = Path::new("/work/one/skills");
        assert_eq!(
            workspace_root_from(None, None, Some(home), skills),
            workspace_root_from(None, None, Some(home), skills)
        );
    }

    #[test]
    fn workspace_root_slug_survives_a_basename_that_is_not_path_safe() {
        let root = workspace_root_from(
            None,
            None,
            Some(Path::new("/home/u")),
            Path::new("/work/my skills:v2"),
        );
        let slug = root.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
            "slug was {slug}"
        );
        assert!(slug.starts_with("my-skills-v2-"), "slug was {slug}");
    }

    /// An operator upgrading mid-campaign would otherwise find `ingest` unable to
    /// see the iteration `run` had just built.
    #[test]
    fn a_legacy_workspace_in_the_cwd_is_reported() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        fs::create_dir_all(tmp.path().join(".eval-magic").join("foo")).unwrap();

        let ctx = detect_run_context(DetectInput {
            cwd: Some(tmp.path().to_path_buf()),
            ..input(&skill_dir, "foo")
        })
        .unwrap();

        assert!(
            ctx.warnings.iter().any(|w| w.contains(".eval-magic")),
            "warnings were: {:?}",
            ctx.warnings
        );
    }

    /// Advice to "pass --workspace-dir <x>" is worse than silence when the run is
    /// already using `<x>`. Reachable whenever the resolved root lands on the old
    /// path — an `EVAL_MAGIC_WORKSPACE_DIR` naming it, say.
    #[test]
    fn no_legacy_notice_when_the_resolved_workspace_is_that_directory() {
        let tmp = TempDir::new().unwrap();
        let legacy = tmp.path().join(".eval-magic");
        fs::create_dir_all(legacy.join("mr-review")).unwrap();

        assert_eq!(legacy_workspace_notice(tmp.path(), &legacy), None);
        assert!(legacy_workspace_notice(tmp.path(), Path::new("/elsewhere/eval-magic")).is_some());
    }

    /// `.eval-magic/harnesses/` is the project-local descriptor layer — a
    /// deliberate, unrelated use of the same name that does not move and must
    /// not be mistaken for an orphaned campaign.
    #[test]
    fn a_descriptor_layer_alone_is_not_reported_as_a_legacy_workspace() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        fs::create_dir_all(tmp.path().join(".eval-magic").join("harnesses")).unwrap();

        let ctx = detect_run_context(DetectInput {
            cwd: Some(tmp.path().to_path_buf()),
            ..input(&skill_dir, "foo")
        })
        .unwrap();

        assert!(ctx.warnings.is_empty(), "warnings were: {:?}", ctx.warnings);
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
        assert_eq!(
            ctx.workspace_root,
            crate::core::fs::real_path(&custom).unwrap()
        );
    }

    #[test]
    fn stage_root_default() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let ctx = detect_run_context(input(&skill_dir, "foo")).unwrap();
        assert_eq!(
            ctx.stage_root,
            crate::core::fs::real_path(&std::env::current_dir().unwrap()).unwrap()
        );
    }

    /// Every root derives from the cwd, and the guard later compares those roots
    /// against paths the agent's own tools report — so an alias of the cwd has to
    /// collapse here, once, or the two sides disagree forever after.
    ///
    /// Windows spells one directory several ways (8.3 short names, junctions,
    /// `subst` drives, redirected profiles); each is one `canonicalize` apart
    /// from the real path, so exercising one exercises the mechanism.
    #[test]
    fn a_cwd_alias_collapses_so_every_derived_root_shares_one_spelling() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real-workspace");
        fs::create_dir_all(&real).unwrap();
        let alias = tmp.path().join("alias-workspace");
        crate::core::fs::create_directory_alias(&real, &alias).unwrap();
        make_skill_dir(&real, &["foo"]);

        // Enter through the alias, exactly as a user whose workspace sits under a
        // junction or a redirected profile directory does.
        let ctx = detect_run_context(DetectInput {
            skill: Some("foo".to_string()),
            ..input_from(&alias.join("skill-dir"))
        })
        .unwrap();

        let expected = crate::core::fs::real_path(&real).unwrap();
        assert_eq!(ctx.stage_root, expected.join("skill-dir"));
        assert_eq!(ctx.skill_dir, expected.join("skill-dir"));

        // The workspace root now derives from the skill dir rather than the cwd,
        // so the alias has to collapse there too: entering through the alias and
        // entering directly must name one workspace, not two.
        let direct = detect_run_context(DetectInput {
            skill: Some("foo".to_string()),
            ..input_from(&expected.join("skill-dir"))
        })
        .unwrap();
        assert_eq!(ctx.workspace_root, direct.workspace_root);
    }

    /// `--workspace-dir` is the second way into the same tree: the guard's roots
    /// descend from it, so an alias passed here would reintroduce the split the
    /// cwd resolution just closed.
    #[test]
    fn an_aliased_workspace_dir_flag_resolves_to_the_same_spelling() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real-workspace");
        fs::create_dir_all(&real).unwrap();
        let alias = tmp.path().join("alias-workspace");
        crate::core::fs::create_directory_alias(&real, &alias).unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);

        let ctx = detect_run_context(DetectInput {
            workspace_dir: Some(alias.join("nested-ws").to_string_lossy().into_owned()),
            ..input(&skill_dir, "foo")
        })
        .unwrap();

        assert_eq!(
            ctx.workspace_root,
            crate::core::fs::real_path(&real).unwrap().join("nested-ws")
        );
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
            Some(crate::core::fs::real_path(&bootstrap).unwrap())
        );
    }

    #[test]
    fn harness_codex_accepted() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let ctx = detect_run_context(DetectInput {
            harness: Some(Harness::resolve("codex").unwrap()),
            ..input(&skill_dir, "foo")
        })
        .unwrap();
        assert_eq!(ctx.harness, Harness::resolve("codex").unwrap());
    }

    #[test]
    fn harness_opencode_accepted() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
        let ctx = detect_run_context(DetectInput {
            harness: Some(Harness::resolve("opencode").unwrap()),
            ..input(&skill_dir, "foo")
        })
        .unwrap();
        assert_eq!(ctx.harness, Harness::resolve("opencode").unwrap());
    }
}
