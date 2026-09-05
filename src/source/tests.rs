//! Tests for [`super`]: resolving a declared source and materializing it.
//!
//! Extracted from `mod.rs` because the module outgrew the file it exercised
//! — the convention in CLAUDE.md, whose trigger is size rather than style.

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
        "codebase",
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
        "codebase",
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
        "codebase",
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
        "codebase",
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
        "codebase",
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
        "codebase",
    )
    .expect_err("an unresolvable ref fails")
    .to_string();

    assert!(error.contains("no-such-branch"), "error was: {error}");
    assert!(
        error.contains(&origin.to_string_lossy().into_owned()),
        "error was: {error}"
    );
}

/// The module resolves whatever it is handed, so the noun in its messages is
/// the caller's to supply. Without this the skill path reports itself as a
/// codebase, which is the one thing the operator cannot act on.
#[test]
fn a_failure_is_reported_in_the_subject_the_caller_named() {
    let tmp = tempfile::TempDir::new().unwrap();

    let error = resolve(
        &SourceSpec::Path {
            path: "no-such-directory".to_string(),
        },
        tmp.path(),
        "skill",
    )
    .expect_err("a path that does not exist cannot resolve");

    let message = error.to_string();
    assert!(message.contains("skill path"), "message was: {message}");
    assert!(!message.contains("codebase"), "message was: {message}");
}

/// A warning is advice; the flag is evidence. A skill is copied as it sits on
/// disk, so whether the tree was dirty changes what the run measured and has
/// to survive into the artifacts rather than only into stderr.
#[test]
fn path_source_records_an_uncommitted_tree_as_dirty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let local = source_repo(tmp.path(), "local", "main");
    std::fs::write(local.join("README.md"), "edited but never committed\n").unwrap();

    let resolved = resolve(
        &SourceSpec::Path {
            path: local.to_string_lossy().into_owned(),
        },
        tmp.path(),
        "codebase",
    )
    .expect("a dirty repository still resolves");

    assert!(resolved.dirty, "an uncommitted edit makes the tree dirty");
}

#[test]
fn path_source_records_a_clean_tree_as_not_dirty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let local = source_repo(tmp.path(), "local", "main");

    let resolved = resolve(
        &SourceSpec::Path {
            path: local.to_string_lossy().into_owned(),
        },
        tmp.path(),
        "codebase",
    )
    .expect("a clean repository resolves");

    assert!(!resolved.dirty);
}

/// A skill is a subdirectory of a repository that holds many of them. Reporting
/// the whole repository's status would call every skill dirty the moment any
/// other one was edited, which is worse than not reporting at all.
#[test]
fn a_subdirectory_source_ignores_uncommitted_changes_elsewhere_in_the_repository() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = source_repo(tmp.path(), "skills", "main");
    let subject = repo.join("mr-review");
    std::fs::create_dir_all(&subject).unwrap();
    std::fs::write(subject.join("SKILL.md"), "subject\n").unwrap();
    commit(
        &repo,
        "unrelated.md",
        "add a sibling file and the subject skill",
    );
    std::fs::write(repo.join("unrelated.md"), "edited elsewhere\n").unwrap();

    let resolved = resolve(
        &SourceSpec::Path {
            path: subject.to_string_lossy().into_owned(),
        },
        tmp.path(),
        "skill",
    )
    .expect("a subdirectory of a repository resolves");

    assert!(
        !resolved.dirty,
        "an edit outside the subject subtree is not the subject's dirtiness"
    );
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
        "codebase",
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
        "codebase",
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
        "codebase",
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
        "codebase",
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

/// The environment a run provisions from its cached materialization:
/// a local clone while the cache carries check-outable history and the
/// host allows hard links, a plain copy otherwise.
#[test]
fn provisioning_an_env_from_a_cached_checkout_clones_with_history_and_no_remote() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = source_repo(tmp.path(), "origin", "main");
    commit(&origin, "second.txt", "second");
    let resolved = resolve(
        &SourceSpec::Git {
            url: origin.to_string_lossy().into_owned(),
            reference: "main".to_string(),
        },
        tmp.path(),
        "codebase",
    )
    .unwrap();
    let cache = tmp.path().join("cache");
    materialize(&resolved, &cache).unwrap();
    let env = tmp.path().join("env");

    let outcome = provision_env(&resolved, &cache, &env).expect("provisioning succeeds");

    assert!(
        matches!(outcome, EnvProvisioning::LocalClone),
        "a cached checkout with commits and a hard-linking host clones"
    );
    assert_eq!(sha(&env, "HEAD"), resolved.revision.unwrap());
    assert_eq!(git_text(&env, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(
        git_text(&env, &["rev-list", "--count", "HEAD"]),
        "2",
        "a local clone carries the cached checkout's history"
    );
    assert_eq!(
        git_text(&env, &["remote"]),
        "",
        "a local clone names its source as a remote, so provisioning removes it"
    );
    assert_eq!(git_text(&env, &["status", "--porcelain"]), "");
    assert_eq!(
        std::fs::read_to_string(env.join("second.txt")).unwrap(),
        "second
"
    );
}

/// A cache materialized from a directory with no Git history has no commits,
/// and `git clone` of an empty repository checks out nothing — the only
/// correct provisioning there is the plain copy.
#[test]
fn provisioning_a_commitless_cache_falls_back_to_a_plain_copy() {
    let tmp = tempfile::TempDir::new().unwrap();
    let plain = tmp.path().join("plain-project");
    std::fs::create_dir_all(plain.join("src")).unwrap();
    std::fs::write(
        plain.join("src/main.rs"),
        "fn main() {}
",
    )
    .unwrap();
    let resolved = resolve(
        &SourceSpec::Path {
            path: plain.to_string_lossy().into_owned(),
        },
        tmp.path(),
        "codebase",
    )
    .unwrap();
    let cache = tmp.path().join("cache");
    materialize(&resolved, &cache).unwrap();
    let env = tmp.path().join("env");

    let outcome = provision_env(&resolved, &cache, &env).expect("provisioning succeeds");

    assert!(matches!(outcome, EnvProvisioning::PlainCopy));
    assert_eq!(
        std::fs::read_to_string(env.join("src/main.rs")).unwrap(),
        "fn main() {}
",
        "a commitless cache must still populate the environment's working tree"
    );
}

/// Environments share the cache's objects by hard link, but
/// each keeps a private working tree and private refs — a
/// write in one is invisible to the others and to the cache.
#[test]
fn envs_provisioned_from_one_cache_are_independent_working_trees() {
    let tmp = tempfile::TempDir::new().unwrap();
    let origin = source_repo(tmp.path(), "origin", "main");
    let resolved = resolve(
        &SourceSpec::Git {
            url: origin.to_string_lossy().into_owned(),
            reference: "main".to_string(),
        },
        tmp.path(),
        "codebase",
    )
    .unwrap();
    let cache = tmp.path().join("cache");
    materialize(&resolved, &cache).unwrap();
    let env_one = tmp.path().join("env-one");
    let env_two = tmp.path().join("env-two");
    provision_env(&resolved, &cache, &env_one).unwrap();
    provision_env(&resolved, &cache, &env_two).unwrap();

    commit(&env_one, "agent-work.txt", "work committed in env one");

    assert_eq!(
        git_text(&env_one, &["rev-list", "--count", "HEAD"]),
        "2",
        "the writing environment moved ahead on its own history"
    );
    assert_eq!(
        git_text(&env_two, &["rev-list", "--count", "HEAD"]),
        "1",
        "the other environment's history is untouched"
    );
    assert_eq!(
        git_text(&cache, &["rev-list", "--count", "HEAD"]),
        "1",
        "the cache every environment shares is never mutated"
    );
    assert!(!env_two.join("agent-work.txt").exists());
    assert!(!cache.join("agent-work.txt").exists());
}
