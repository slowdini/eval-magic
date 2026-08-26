//! Resolving a declared source to a revision, materializing it as a tree, and
//! provisioning task environments from that tree.
//!
//! Three operations, deliberately split. [`resolve`] is read-only: it answers
//! "what exactly does this declaration point at?" without creating a directory,
//! so a run can fail on an unreachable repository or a ref that does not exist
//! before it has built any part of a workspace. [`materialize`] creates the one
//! cached checkout an iteration shares; [`provision_env`] turns that cache into
//! each individual task environment.
//!
//! Nothing here knows what a codebase is. A caller hands it a [`SourceSpec`] and
//! gets back a [`ResolvedSource`]; the eval config's `codebase` block is one
//! producer of that spec.

use std::path::Path;

use crate::core::IsolatedGit;

/// Branch a source that carries no Git history of its own is initialized on.
pub const INITIALIZED_BRANCH: &str = "work";

/// A declared source, independent of what it is being sourced *for*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSpec {
    Git {
        url: String,
        reference: String,
    },
    /// A directory on this host. Relative paths resolve against the `base_dir`
    /// handed to [`resolve`] — for an eval config, the directory holding it.
    Path {
        path: String,
    },
}

/// The read-only outcome of [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSource {
    /// The noun every message about this resolution uses. The caller names it,
    /// because this module deliberately does not know what it is sourcing.
    pub subject: &'static str,
    /// The url or path exactly as declared.
    pub source: String,
    /// The absolute directory a path source resolved to. Absent for a git url,
    /// which names no directory on this host.
    pub resolved_path: Option<String>,
    /// The declared ref, for a git source.
    pub reference: Option<String>,
    /// The commit the declaration resolves to. `None` when the source is a
    /// directory that is not a Git repository — there is no commit to name.
    pub revision: Option<String>,
    /// The source repository's `origin`, when it has one. Recorded because it is
    /// the only reproducible handle a host-local path source can offer: another
    /// reader cannot resolve the path, but can resolve `origin` + `revision`.
    pub origin_url: Option<String>,
    /// Branch a materialized copy checks out.
    pub branch: String,
    /// True when the declaration cannot be resolved off this host, so a report
    /// citing it is not reproducible from the config alone.
    pub host_local: bool,
    /// True when the source tree carries uncommitted changes. A warning is
    /// advice the operator may miss; this is evidence, and a subject copied as
    /// it sits on disk cannot be cited without it.
    pub dirty: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("{0}")]
    Message(String),
}

impl SourceError {
    fn msg(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

/// Resolve `spec` without creating anything on disk. `subject` is the noun the
/// caller wants this resolution's messages to use — `codebase`, `skill`.
pub fn resolve(
    spec: &SourceSpec,
    base_dir: &Path,
    subject: &'static str,
) -> Result<ResolvedSource, SourceError> {
    match spec {
        SourceSpec::Git { url, reference } => resolve_git(url, reference, subject),
        SourceSpec::Path { path } => resolve_path(path, base_dir, subject),
    }
}

fn resolve_path(
    declared: &str,
    base_dir: &Path,
    subject: &'static str,
) -> Result<ResolvedSource, SourceError> {
    let joined = {
        let path = Path::new(declared);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base_dir.join(path)
        }
    };
    let directory = crate::core::fs::real_path(&joined).map_err(|error| {
        SourceError::msg(format!(
            "{subject} path '{declared}' could not be resolved ({}): {error}",
            joined.display()
        ))
    })?;
    if !directory.is_dir() {
        return Err(SourceError::msg(format!(
            "{subject} path '{declared}' is not a directory: {}",
            directory.display()
        )));
    }

    let git = IsolatedGit::new().map_err(SourceError::msg)?;
    let text = |args: &[&str]| {
        let output = git.run(&directory, args, &[]);
        (output.status == Some(0))
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty())
    };

    // Reported, not interpreted: what a dirty tree *means* differs by subject —
    // a codebase leaves the work behind at checkout, a skill is copied carrying
    // it — so the caller phrases the consequence it owns.
    //
    // `-- .` scopes the probe to the resolved directory's own subtree. A skill is
    // one directory among many in a repository; reporting the repository's status
    // would call it dirty the moment any *other* skill was edited.
    let dirty = text(&["status", "--porcelain", "--", "."]).is_some();

    Ok(ResolvedSource {
        subject,
        source: declared.to_string(),
        resolved_path: Some(directory.to_string_lossy().into_owned()),
        reference: None,
        revision: text(&["rev-parse", "HEAD"]),
        origin_url: text(&["remote", "get-url", "origin"]),
        // A detached HEAD reports no symbolic ref either; both it and a plain
        // directory land on the branch a fresh `git init` would have created.
        branch: text(&["symbolic-ref", "--short", "HEAD"])
            .unwrap_or_else(|| INITIALIZED_BRANCH.to_string()),
        host_local: true,
        dirty,
    })
}

fn resolve_git(
    url: &str,
    reference: &str,
    subject: &'static str,
) -> Result<ResolvedSource, SourceError> {
    let refs = list_remote(url, subject)?;
    let value_of = |name: &str| {
        refs.iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.clone())
    };

    // A branch keeps its own name; anything else lands on the repository's
    // default branch, since a tag or a bare SHA names no branch to be on.
    let branch_ref = format!("refs/heads/{reference}");
    let (revision, branch) = match value_of(&branch_ref) {
        Some(revision) => (revision, reference.to_string()),
        None => {
            // `refs/tags/<x>^{}` is the commit an annotated tag peels to; a
            // lightweight tag advertises only the unpeeled name, which already
            // is a commit.
            let tagged = value_of(&format!("refs/tags/{reference}^{{}}"))
                .or_else(|| value_of(&format!("refs/tags/{reference}")));
            // A remote advertises refs, not arbitrary commits, so a SHA matches
            // nothing above and is taken at face value. Materialization is what
            // proves it exists — it fails loudly there if it does not.
            let revision = tagged
                .or_else(|| is_full_sha(reference).then(|| reference.to_string()))
                .ok_or_else(|| {
                    SourceError::msg(format!(
                        "{subject} ref '{reference}' does not exist in {url}"
                    ))
                })?;
            (revision, default_branch(&refs, url)?)
        }
    };

    Ok(ResolvedSource {
        subject,
        source: url.to_string(),
        resolved_path: None,
        reference: Some(reference.to_string()),
        revision: Some(revision),
        // The url *is* the origin, and materialization strips the remote, so
        // recording it here keeps the pointer the stripped remote would have been.
        origin_url: Some(url.to_string()),
        branch,
        host_local: false,
        // A clone takes a named commit; there is no working tree to be dirty.
        dirty: false,
    })
}

/// Materialize `resolved` into `dest`, which must not already exist.
///
/// A source with history is cloned, so the history arrives with it; a plain
/// directory is copied and initialized. Either way `dest` ends up a Git
/// repository, checked out on [`ResolvedSource::branch`], with no remote — a
/// task environment must not be able to reach the source it came from.
pub fn materialize(resolved: &ResolvedSource, dest: &Path) -> Result<(), SourceError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            SourceError::msg(format!("could not create {}: {error}", parent.display()))
        })?;
    }

    let git = IsolatedGit::new().map_err(SourceError::msg)?;
    match (&resolved.resolved_path, &resolved.revision) {
        // A directory carrying no history: copy it, then wrap it in a repository.
        (Some(directory), None) => {
            crate::core::fs::copy_entry_materialized(Path::new(directory), dest).map_err(
                |error| {
                    SourceError::msg(format!(
                        "could not copy {} directory {directory} into {}: {error}",
                        resolved.subject,
                        dest.display()
                    ))
                },
            )?;
            checked(
                &git,
                dest.parent().unwrap_or(dest),
                &[
                    "init",
                    "--quiet",
                    "--initial-branch",
                    &resolved.branch,
                    // An empty template, so a configured `init.templateDir`
                    // cannot seed hooks into a task repository.
                    "--template",
                    &git.template_dir().to_string_lossy(),
                    &dest.to_string_lossy(),
                ],
                &format!(
                    "initialize the {} directory as a repository",
                    resolved.subject
                ),
            )?;
        }
        _ => clone_repository(&git, resolved, dest)?,
    }
    Ok(())
}

fn clone_repository(
    git: &IsolatedGit,
    resolved: &ResolvedSource,
    dest: &Path,
) -> Result<(), SourceError> {
    let from = resolved
        .resolved_path
        .clone()
        .unwrap_or_else(|| resolved.source.clone());
    let revision = resolved.revision.as_deref().ok_or_else(|| {
        SourceError::msg(format!(
            "{} {from} resolved to no commit to check out",
            resolved.subject
        ))
    })?;

    // `--no-checkout` skips populating the working tree at the remote's default
    // branch only to replace it a moment later.
    checked(
        git,
        Path::new("."),
        &[
            "clone",
            "--quiet",
            "--no-checkout",
            // An empty template, for the same reason `init` uses one.
            "--template",
            &git.template_dir().to_string_lossy(),
            &from,
            &dest.to_string_lossy(),
        ],
        &format!("clone {} {from}", resolved.subject),
    )?;
    // `-B` both creates the branch at the resolved commit and checks it out, so a
    // tag or bare SHA never leaves the environment on a detached HEAD.
    checked(
        git,
        dest,
        &["checkout", "--quiet", "-B", &resolved.branch, revision],
        &format!("check out {revision} of {} {from}", resolved.subject),
    )?;
    checked(
        git,
        dest,
        &["remote", "remove", "origin"],
        "remove the cloned remote",
    )?;
    Ok(())
}

/// How [`provision_env`] produced an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvProvisioning {
    /// `git clone --local` from the cache: Git hard-links the object store and
    /// checks out a fresh working tree, so the history arrives intact while the
    /// cache's bytes are paid for once per iteration, not once per environment.
    LocalClone,
    /// A plain materialized copy of the cache: taken when the host cannot
    /// hard-link between the two directories, or the cache carries no commits
    /// to check out.
    PlainCopy,
}

/// Produce one task environment at `dest` from the cached materialization of
/// `resolved` that [`materialize`] left at `cache`.
///
/// A run materializes each distinct codebase once per iteration and provisions
/// every `(group, condition, run)` environment from that single checkout. A
/// local clone is the fast path: Git hard-links the object store instead of
/// copying it, and the clone's history is the checkout's history. A local
/// clone also names its source as an `origin` remote — removed here, so no
/// environment retains a path back to the cache. The plain copy stands in
/// wherever cloning could not deliver: a cache without commits (an empty
/// repository clones to an empty working tree) or a host that refuses the
/// hard link (a cache and an environment on different filesystems).
///
/// `dest` must not already exist. Returns how the environment was provisioned.
pub fn provision_env(
    resolved: &ResolvedSource,
    cache: &Path,
    dest: &Path,
) -> Result<EnvProvisioning, SourceError> {
    if resolved.revision.is_none() {
        copy_from_cache(cache, dest)?;
        return Ok(EnvProvisioning::PlainCopy);
    }
    let git = IsolatedGit::new().map_err(SourceError::msg)?;
    let parent = dest.parent().unwrap_or(dest);
    std::fs::create_dir_all(parent).map_err(|error| {
        SourceError::msg(format!("could not create {}: {error}", parent.display()))
    })?;
    // Probing `cache/.git`, not the working tree: a probe file in the tree
    // would be a leftover in the cache even after deletion racing a clone,
    // while `.git` is metadata no checkout ever reads.
    if !crate::core::fs::hardlinks_available(&cache.join(".git"), parent) {
        copy_from_cache(cache, dest)?;
        return Ok(EnvProvisioning::PlainCopy);
    }
    checked(
        &git,
        Path::new("."),
        &[
            "clone",
            "--quiet",
            "--local",
            "--template",
            &git.template_dir().to_string_lossy(),
            &cache.to_string_lossy(),
            &dest.to_string_lossy(),
        ],
        "clone the cached codebase checkout",
    )?;
    checked(
        &git,
        dest,
        &["remote", "remove", "origin"],
        "remove the cache as a remote",
    )?;
    Ok(EnvProvisioning::LocalClone)
}

/// The fallback provisioning: a materialized copy of the whole cache, for a
/// host that cannot hard-link between the two directories or a cache with no
/// commits to check out.
fn copy_from_cache(cache: &Path, dest: &Path) -> Result<(), SourceError> {
    std::fs::create_dir_all(dest).map_err(|error| {
        SourceError::msg(format!(
            "could not create environment {}: {error}",
            dest.display()
        ))
    })?;
    crate::core::fs::copy_entry_materialized(cache, dest).map_err(|error| {
        SourceError::msg(format!(
            "could not copy cached codebase {} into {}: {error}",
            cache.display(),
            dest.display()
        ))
    })
}

/// Run git in `cwd`, turning a non-zero exit into an error naming the intent.
fn checked(git: &IsolatedGit, cwd: &Path, args: &[&str], intent: &str) -> Result<(), SourceError> {
    let output = git.run(cwd, args, &[]);
    if output.status == Some(0) {
        return Ok(());
    }
    Err(SourceError::msg(format!(
        "could not {intent}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// Whether `reference` is a full 40-character object name.
///
/// Only the full form. An abbreviated SHA cannot be distinguished from a branch
/// named `abc1234`, and guessing wrong would silently source the wrong tree.
fn is_full_sha(reference: &str) -> bool {
    reference.len() == 40 && reference.chars().all(|c| c.is_ascii_hexdigit())
}

/// The branch `HEAD` points at on the remote, from the `--symref` line.
fn default_branch(refs: &[(String, String)], url: &str) -> Result<String, SourceError> {
    refs.iter()
        .find(|(name, value)| name == "HEAD" && value.starts_with("ref: refs/heads/"))
        .and_then(|(_, value)| value.strip_prefix("ref: refs/heads/"))
        .map(str::to_string)
        .ok_or_else(|| {
            SourceError::msg(format!(
                "could not determine the default branch of {url}; it advertises no HEAD symref"
            ))
        })
}

/// `(ref name, value)` pairs advertised by `url`, including the `HEAD` symref.
///
/// Deliberately unfiltered. Passing a ref pattern makes `ls-remote` list only
/// matching refs, which drops the `ref: refs/heads/<x>\tHEAD` line — and that
/// line is the only way to learn the remote's default branch. One unfiltered
/// call answers both questions in one round trip.
fn list_remote(url: &str, subject: &'static str) -> Result<Vec<(String, String)>, SourceError> {
    let git = IsolatedGit::new().map_err(SourceError::msg)?;
    let output = git.run(Path::new("."), &["ls-remote", "--symref", url], &[]);
    if output.status != Some(0) {
        return Err(SourceError::msg(format!(
            "could not read {subject} repository {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(value, name)| (name.trim().to_string(), value.trim().to_string()))
        .collect())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
