//! Resolving a declared source to a revision, and materializing it as a tree.
//!
//! Two phases, deliberately split. [`resolve`] is read-only: it answers "what
//! exactly does this declaration point at?" without creating a directory, so a
//! run can fail on an unreachable repository or a ref that does not exist before
//! it has built any part of a workspace.
//!
//! Nothing here knows what a codebase is. A caller hands it a [`SourceSpec`] and
//! gets back a [`ResolvedSource`]; the eval config's `codebase` block is one
//! producer of that spec.

use std::path::Path;

use crate::core::run_git;

/// Branch a source that carries no Git history of its own is initialized on.
/// Matches the branch a fixture-only task repository has always used, so a run
/// without a codebase looks the same as it always did.
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
    /// Things the operator should know about what this resolution did or did not
    /// carry. This module never prints; the `cli` layer owns the `⚠ ` prefix.
    pub warnings: Vec<String>,
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

/// Resolve `spec` without creating anything on disk.
pub fn resolve(spec: &SourceSpec, base_dir: &Path) -> Result<ResolvedSource, SourceError> {
    match spec {
        SourceSpec::Git { url, reference } => resolve_git(url, reference),
        SourceSpec::Path { path } => resolve_path(path, base_dir),
    }
}

fn resolve_path(declared: &str, base_dir: &Path) -> Result<ResolvedSource, SourceError> {
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
            "codebase path '{declared}' could not be resolved ({}): {error}",
            joined.display()
        ))
    })?;
    if !directory.is_dir() {
        return Err(SourceError::msg(format!(
            "codebase path '{declared}' is not a directory: {}",
            directory.display()
        )));
    }

    let text = |args: &[&str]| {
        let output = run_git(args, &directory);
        (output.status == Some(0))
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty())
    };

    // Materialization takes a clean checkout of HEAD, so anything uncommitted in
    // the source is not carried. That is the chosen behavior, not a bug — but it
    // is invisible from the task environment, so it is said out loud here.
    let mut warnings = Vec::new();
    if text(&["status", "--porcelain"]).is_some() {
        warnings.push(format!(
            "codebase path '{declared}' has uncommitted changes; the task environment is a clean \
             checkout of its committed state and does not include them"
        ));
    }

    Ok(ResolvedSource {
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
        warnings,
    })
}

fn resolve_git(url: &str, reference: &str) -> Result<ResolvedSource, SourceError> {
    let refs = list_remote(url)?;
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
                        "codebase ref '{reference}' does not exist in {url}"
                    ))
                })?;
            (revision, default_branch(&refs, url)?)
        }
    };

    Ok(ResolvedSource {
        source: url.to_string(),
        resolved_path: None,
        reference: Some(reference.to_string()),
        revision: Some(revision),
        // The url *is* the origin, and materialization strips the remote, so
        // recording it here keeps the pointer the stripped remote would have been.
        origin_url: Some(url.to_string()),
        branch,
        host_local: false,
        warnings: Vec::new(),
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

    match (&resolved.resolved_path, &resolved.revision) {
        // A directory carrying no history: copy it, then wrap it in a repository.
        (Some(directory), None) => {
            crate::core::fs::copy_entry_materialized(Path::new(directory), dest).map_err(
                |error| {
                    SourceError::msg(format!(
                        "could not copy codebase directory {directory} into {}: {error}",
                        dest.display()
                    ))
                },
            )?;
            checked(
                dest.parent().unwrap_or(dest),
                &[
                    "init",
                    "--quiet",
                    "--initial-branch",
                    &resolved.branch,
                    &dest.to_string_lossy(),
                ],
                "initialize the codebase directory as a repository",
            )?;
        }
        _ => clone_repository(resolved, dest)?,
    }
    Ok(())
}

fn clone_repository(resolved: &ResolvedSource, dest: &Path) -> Result<(), SourceError> {
    let from = resolved
        .resolved_path
        .clone()
        .unwrap_or_else(|| resolved.source.clone());
    let revision = resolved.revision.as_deref().ok_or_else(|| {
        SourceError::msg(format!(
            "codebase {from} resolved to no commit to check out"
        ))
    })?;

    // `--no-checkout` skips populating the working tree at the remote's default
    // branch only to replace it a moment later.
    checked(
        Path::new("."),
        &[
            "clone",
            "--quiet",
            "--no-checkout",
            &from,
            &dest.to_string_lossy(),
        ],
        &format!("clone codebase {from}"),
    )?;
    // `-B` both creates the branch at the resolved commit and checks it out, so a
    // tag or bare SHA never leaves the environment on a detached HEAD.
    checked(
        dest,
        &["checkout", "--quiet", "-B", &resolved.branch, revision],
        &format!("check out {revision} of codebase {from}"),
    )?;
    checked(
        dest,
        &["remote", "remove", "origin"],
        "remove the cloned remote",
    )?;
    Ok(())
}

/// Run git in `cwd`, turning a non-zero exit into an error naming the intent.
fn checked(cwd: &Path, args: &[&str], intent: &str) -> Result<(), SourceError> {
    let output = run_git(args, cwd);
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
fn list_remote(url: &str) -> Result<Vec<(String, String)>, SourceError> {
    let output = run_git(&["ls-remote", "--symref", url], Path::new("."));
    if output.status != Some(0) {
        return Err(SourceError::msg(format!(
            "could not read codebase repository {url}: {}",
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
mod tests {
    use super::*;

    use std::path::{Path, PathBuf};

    use crate::core::run_git;

    /// A repository at `name` with one commit on `branch`, usable as a clone URL.
    fn source_repo(root: &Path, name: &str, branch: &str) -> PathBuf {
        let repo = root.join(name);
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&["init", "--quiet", "--initial-branch", branch, "."], &repo);
        std::fs::write(repo.join("README.md"), "source\n").unwrap();
        run_git(&["add", "--all"], &repo);
        run_git(
            &[
                "-c",
                "user.name=source",
                "-c",
                "user.email=source@localhost",
                "commit",
                "--quiet",
                "--no-gpg-sign",
                "-m",
                "initial",
            ],
            &repo,
        );
        repo
    }

    /// The commit `revision` names in `repo`.
    fn sha(repo: &Path, revision: &str) -> String {
        let out = run_git(&["rev-parse", revision], repo);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Trimmed stdout of a git invocation in `repo`.
    fn git_text(repo: &Path, args: &[&str]) -> String {
        let out = run_git(args, repo);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Add one more commit touching `file`, so history has depth to preserve.
    fn commit(repo: &Path, file: &str, message: &str) {
        std::fs::write(repo.join(file), format!("{message}\n")).unwrap();
        run_git(&["add", "--all"], repo);
        run_git(
            &[
                "-c",
                "user.name=source",
                "-c",
                "user.email=source@localhost",
                "commit",
                "--quiet",
                "--no-gpg-sign",
                "-m",
                message,
            ],
            repo,
        );
    }

    #[test]
    fn git_source_resolves_a_branch_ref_to_its_commit_and_default_branch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let origin = source_repo(tmp.path(), "origin", "main");

        let resolved = resolve(
            &SourceSpec::Git {
                url: origin.to_string_lossy().into_owned(),
                reference: "main".to_string(),
            },
            tmp.path(),
        )
        .expect("a branch ref on a reachable repository resolves");

        assert_eq!(
            resolved.revision.as_deref(),
            Some(sha(&origin, "main").as_str())
        );
        assert_eq!(resolved.branch, "main");
        assert!(
            !resolved.host_local,
            "a git url is reproducible from the config alone"
        );
    }

    /// A tag names no branch, so the checkout has to land somewhere. It lands on
    /// the repository's *own* default branch — which is only knowable from the
    /// `HEAD` symref line, and `ls-remote` suppresses that line when a ref
    /// pattern is passed. This test is what holds the unfiltered call in place.
    #[test]
    fn git_source_resolves_an_annotated_tag_to_its_commit_on_the_default_branch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let origin = source_repo(tmp.path(), "origin", "trunk");
        run_git(
            &[
                "-c",
                "user.name=source",
                "-c",
                "user.email=source@localhost",
                "tag",
                "--annotate",
                "v1",
                "-m",
                "release",
            ],
            &origin,
        );

        let resolved = resolve(
            &SourceSpec::Git {
                url: origin.to_string_lossy().into_owned(),
                reference: "v1".to_string(),
            },
            tmp.path(),
        )
        .expect("an annotated tag resolves");

        assert_eq!(
            resolved.revision.as_deref(),
            Some(sha(&origin, "v1^{commit}").as_str()),
            "an annotated tag must resolve to the commit it peels to"
        );
        assert_ne!(
            resolved.revision.as_deref(),
            Some(sha(&origin, "v1").as_str()),
            "the tag object is not a commit and cannot be checked out as one"
        );
        assert_eq!(resolved.branch, "trunk");
    }

    #[test]
    fn path_source_that_is_a_repository_records_its_revision_origin_and_branch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let upstream = source_repo(tmp.path(), "upstream", "main");
        let local = source_repo(tmp.path(), "local", "feature");
        run_git(
            &["remote", "add", "origin", &upstream.to_string_lossy()],
            &local,
        );

        // Declared relative, so this also pins resolution against `base_dir`.
        let resolved = resolve(
            &SourceSpec::Path {
                path: "local".to_string(),
            },
            tmp.path(),
        )
        .expect("a local repository resolves");

        assert_eq!(
            resolved.revision.as_deref(),
            Some(sha(&local, "HEAD").as_str())
        );
        assert_eq!(resolved.branch, "feature");
        assert!(
            resolved.host_local,
            "a path names a directory only this host has"
        );
        // The origin is what makes a host-local source citable elsewhere:
        // `origin` + `revision` is reproducible even though `path` is not.
        assert_eq!(
            resolved.origin_url.as_deref(),
            Some(upstream.to_string_lossy().as_ref())
        );
    }

    /// The ticket's second acceptance criterion: a plain directory still has to
    /// yield a working task repository, so it resolves rather than failing —
    /// with no commit to name, on the branch a fresh `git init` will create.
    #[test]
    fn path_source_that_is_not_a_repository_resolves_without_a_revision() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plain = tmp.path().join("plain-project");
        std::fs::create_dir_all(plain.join("src")).unwrap();
        std::fs::write(plain.join("src/main.rs"), "fn main() {}\n").unwrap();

        let resolved = resolve(
            &SourceSpec::Path {
                path: plain.to_string_lossy().into_owned(),
            },
            tmp.path(),
        )
        .expect("a directory that is not a repository still resolves");

        assert_eq!(resolved.revision, None, "a plain directory names no commit");
        assert_eq!(resolved.origin_url, None);
        assert_eq!(resolved.branch, INITIALIZED_BRANCH);
        assert!(resolved.host_local);
    }

    /// A remote advertises refs, not arbitrary commits, so a SHA matches nothing
    /// in `ls-remote` and is taken at face value here; the clone proves it exists.
    #[test]
    fn git_source_accepts_a_full_sha_ref_on_the_default_branch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let origin = source_repo(tmp.path(), "origin", "trunk");
        let head = sha(&origin, "HEAD");

        let resolved = resolve(
            &SourceSpec::Git {
                url: origin.to_string_lossy().into_owned(),
                reference: head.clone(),
            },
            tmp.path(),
        )
        .expect("a full commit SHA resolves");

        assert_eq!(resolved.revision.as_deref(), Some(head.as_str()));
        assert_eq!(resolved.branch, "trunk");
    }

    #[test]
    fn git_source_ref_that_does_not_exist_names_the_ref_and_the_url() {
        let tmp = tempfile::TempDir::new().unwrap();
        let origin = source_repo(tmp.path(), "origin", "main");

        let error = resolve(
            &SourceSpec::Git {
                url: origin.to_string_lossy().into_owned(),
                reference: "no-such-branch".to_string(),
            },
            tmp.path(),
        )
        .expect_err("an unresolvable ref fails")
        .to_string();

        assert!(error.contains("no-such-branch"), "error was: {error}");
        assert!(
            error.contains(&origin.to_string_lossy().into_owned()),
            "error was: {error}"
        );
    }

    /// The user chose a clean checkout of HEAD over a verbatim copy, so a dirty
    /// working tree is silently *not* carried. Saying so is what keeps that from
    /// being a surprise.
    #[test]
    fn path_source_with_uncommitted_changes_warns_that_they_are_not_carried() {
        let tmp = tempfile::TempDir::new().unwrap();
        let local = source_repo(tmp.path(), "local", "main");
        std::fs::write(local.join("README.md"), "edited but never committed\n").unwrap();

        let resolved = resolve(
            &SourceSpec::Path {
                path: local.to_string_lossy().into_owned(),
            },
            tmp.path(),
        )
        .expect("a dirty repository still resolves");

        assert!(
            resolved
                .warnings
                .iter()
                .any(|warning| warning.contains("uncommitted")),
            "warnings were: {:?}",
            resolved.warnings
        );
    }

    #[test]
    fn path_source_with_a_clean_tree_warns_about_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let local = source_repo(tmp.path(), "local", "main");

        let resolved = resolve(
            &SourceSpec::Path {
                path: local.to_string_lossy().into_owned(),
            },
            tmp.path(),
        )
        .expect("a clean repository resolves");

        assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    }

    /// The ticket's first acceptance criterion, at the resolver boundary: a real
    /// checkout, history intact, no remotes configured.
    #[test]
    fn materializing_a_git_source_keeps_history_and_configures_no_remote() {
        let tmp = tempfile::TempDir::new().unwrap();
        let origin = source_repo(tmp.path(), "origin", "main");
        commit(&origin, "second.txt", "second");
        let resolved = resolve(
            &SourceSpec::Git {
                url: origin.to_string_lossy().into_owned(),
                reference: "main".to_string(),
            },
            tmp.path(),
        )
        .unwrap();
        let dest = tmp.path().join("materialized");

        materialize(&resolved, &dest).expect("a git source materializes");

        assert_eq!(sha(&dest, "HEAD"), resolved.revision.unwrap());
        assert_eq!(
            git_text(&dest, &["symbolic-ref", "--short", "HEAD"]),
            "main"
        );
        assert_eq!(
            git_text(&dest, &["rev-list", "--count", "HEAD"]),
            "2",
            "the clone must carry the source's history, not a squashed snapshot"
        );
        assert_eq!(
            git_text(&dest, &["remote"]),
            "",
            "a task environment must not be able to reach the source it came from"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("second.txt")).unwrap(),
            "second\n"
        );
    }

    /// A tag checks out detached by default; the resolver promised a branch, so
    /// materialization has to put one there.
    #[test]
    fn materializing_a_tag_lands_on_the_default_branch_not_a_detached_head() {
        let tmp = tempfile::TempDir::new().unwrap();
        let origin = source_repo(tmp.path(), "origin", "trunk");
        commit(&origin, "second.txt", "second");
        run_git(
            &[
                "-c",
                "user.name=source",
                "-c",
                "user.email=source@localhost",
                "tag",
                "--annotate",
                "v1",
                "-m",
                "release",
            ],
            &origin,
        );
        let resolved = resolve(
            &SourceSpec::Git {
                url: origin.to_string_lossy().into_owned(),
                reference: "v1".to_string(),
            },
            tmp.path(),
        )
        .unwrap();
        let dest = tmp.path().join("materialized");

        materialize(&resolved, &dest).expect("a tag materializes");

        assert_eq!(
            git_text(&dest, &["symbolic-ref", "--short", "HEAD"]),
            "trunk"
        );
        assert_eq!(sha(&dest, "HEAD"), resolved.revision.unwrap());
    }

    /// The ticket's second acceptance criterion: a plain directory becomes a
    /// working task repository rather than failing for lack of one.
    #[test]
    fn materializing_a_plain_directory_initializes_a_repository_around_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plain = tmp.path().join("plain-project");
        std::fs::create_dir_all(plain.join("src")).unwrap();
        std::fs::write(plain.join("src/main.rs"), "fn main() {}\n").unwrap();
        let resolved = resolve(
            &SourceSpec::Path {
                path: plain.to_string_lossy().into_owned(),
            },
            tmp.path(),
        )
        .unwrap();
        let dest = tmp.path().join("materialized");

        materialize(&resolved, &dest).expect("a plain directory materializes");

        assert_eq!(
            std::fs::read_to_string(dest.join("src/main.rs")).unwrap(),
            "fn main() {}\n"
        );
        assert_eq!(
            git_text(&dest, &["rev-parse", "--is-inside-work-tree"]),
            "true",
            "a plain directory still has to arrive as a repository"
        );
        assert_eq!(
            git_text(&dest, &["symbolic-ref", "--short", "HEAD"]),
            INITIALIZED_BRANCH
        );
    }

    /// A local repository source is a clean checkout of its committed state —
    /// the decision taken on the ticket — so an uncommitted edit is not carried.
    #[test]
    fn materializing_a_dirty_local_repository_carries_only_committed_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let local = source_repo(tmp.path(), "local", "main");
        std::fs::write(local.join("README.md"), "uncommitted\n").unwrap();
        std::fs::write(local.join("untracked.txt"), "untracked\n").unwrap();
        let resolved = resolve(
            &SourceSpec::Path {
                path: local.to_string_lossy().into_owned(),
            },
            tmp.path(),
        )
        .unwrap();
        let dest = tmp.path().join("materialized");

        materialize(&resolved, &dest).expect("a dirty local repository materializes");

        assert_eq!(
            std::fs::read_to_string(dest.join("README.md")).unwrap(),
            "source\n",
            "the committed content, not the working-tree edit"
        );
        assert!(!dest.join("untracked.txt").exists());
        assert_eq!(git_text(&dest, &["remote"]), "");
    }
}
