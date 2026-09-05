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
    /// The absolutized `--harness-file` descriptor this invocation loaded, if
    /// any. Carried so generated follow-up commands can re-emit the flag
    /// (#294) and the iteration can record which descriptor it was prepared
    /// with.
    pub harness_file: Option<PathBuf>,
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
    /// The `--harness-file` this invocation loaded, already absolutized by the
    /// registry init layer; passed through to [`RunContext`] untouched.
    pub harness_file: Option<PathBuf>,
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
    crate::core::fs::fnv1a_hex(path.to_string_lossy().as_bytes())[..8].to_string()
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
        harness_file: input.harness_file,
        warnings,
    })
}

#[cfg(test)]
mod tests;
