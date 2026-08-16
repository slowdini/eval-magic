//! Shared filesystem + JSON helpers — the single home for artifact writing,
//! artifact path rendering, and tree copying, used by `pipeline`, `workspace`,
//! `cli::run`, `adapters`, and `sandbox`.
//!
//! [`artifact_path`] renders a path into the forward-slash wire format every
//! generated artifact carries; [`normalize_separators`] is its comparison-side
//! counterpart, for matching a path spelled by a different host.
//!
//! Copying comes in two flavors. Pick by what the destination is *for*:
//!
//! - [`copy_entry`] mirrors structure, recreating symlinks as symlinks. Right
//!   when the copy must round-trip faithfully — the diff-scope baseline, which
//!   is later compared byte-for-byte against the live tree.
//! - [`copy_entry_materialized`] resolves symlinks into their target's content.
//!   Right for everything else here: staging and fixtures copy *into* an
//!   isolated task env, where a preserved link would point back out of the
//!   sandbox, and snapshots must freeze content so a later run compares against
//!   what was captured.
//!
//! Every function returns [`std::io::Result`], which each consumer error enum
//! (`PipelineError`, `WorkspaceError`, `RunError`) already absorbs via
//! `#[from] std::io::Error` — so call sites keep a bare `?`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Render `path` as the forward-slash string an artifact, manifest, or prompt
/// carries.
///
/// Artifact path fields are a wire format: agents read them, downstream tools
/// join them, and the golden fixtures compare them byte for byte. `Path::join`
/// plus `Display` emits the *host's* separator, so on Windows a POSIX-rooted
/// base yields `/work/cond\run.json` — malformed for every reader. Forward
/// slashes are accepted by the Windows file APIs, so the result stays openable
/// by the stages that read these fields back.
///
/// The rewrite is Windows-only: a POSIX filename may legally contain a literal
/// backslash, and rewriting it there would name a different file. A verbatim
/// (`\\?\`) prefix — what `Path::canonicalize` returns on Windows — is stripped
/// first, since it is an OS escape hatch rather than a path to hand an agent.
///
/// Not for paths handed to a process: a spawned command's argv and the guard
/// hook command line must keep the host's own spelling.
pub fn artifact_path(path: &Path) -> String {
    let rendered = path.to_string_lossy();
    if !cfg!(windows) {
        return rendered.into_owned();
    }
    normalize_separators(&strip_verbatim_prefix(&rendered))
}

/// Drop Windows' verbatim (`\\?\`) prefix, keeping the host's own separators.
fn strip_verbatim_prefix(rendered: &str) -> String {
    match rendered.strip_prefix(r"\\?\UNC\") {
        // Verbatim UNC collapses back to the `\\server\share` form; dropping
        // the whole prefix would leave a bare `UNC\` component.
        Some(rest) => format!(r"\\{rest}"),
        None => rendered
            .strip_prefix(r"\\?\")
            .unwrap_or(rendered)
            .to_string(),
    }
}

/// The one spelling of `path` that every participant in a run agrees on.
///
/// POSIX hands this out for free: `getcwd` resolves symlinks, so a Unix process
/// and everything it spawns already share one spelling of the working directory.
/// Windows makes no such promise — it hands back whatever spelling the cwd was
/// set with — and one directory there has several valid names: an 8.3 short
/// name (`RUNNER~1`), a junction, a `subst` drive, a redirected profile
/// directory. Tools then disagree about which to report: `git` prints the
/// resolved name, while node's `process.cwd()` and `cmd`'s `cd` echo the alias
/// back. Anything comparing those strings — the write guard's allowed roots
/// against the paths an agent's own tools hand it — silently stops matching.
///
/// Resolving once, at the point a run's roots are derived, gives Windows the
/// guarantee POSIX already provides. The verbatim (`\\?\`) prefix comes off
/// because a spawned child reports the plain form, so plain is the spelling the
/// comparisons actually see.
///
/// A run names directories before it creates them, so resolution walks up to the
/// deepest ancestor that exists and re-attaches the rest: the alias always lives
/// in an ancestor — a temp dir, a junction, an 8.3 profile name — never in the
/// leaf about to be created. With no ancestor on disk at all, the lexical form
/// is all there is.
pub fn real_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut unresolved = Vec::new();
    let mut anchor = absolute.as_path();
    loop {
        if let Ok(canonical) = fs::canonicalize(anchor) {
            let mut resolved = if cfg!(windows) {
                PathBuf::from(strip_verbatim_prefix(&canonical.to_string_lossy()))
            } else {
                canonical
            };
            resolved.extend(unresolved.iter().rev());
            return Ok(resolved);
        }
        let (Some(parent), Some(name)) = (anchor.parent(), anchor.file_name()) else {
            return Ok(absolute);
        };
        unresolved.push(name.to_os_string());
        anchor = parent;
    }
}

/// Rewrite Windows separators to forward slashes for *comparison*.
///
/// Unconditional, unlike [`artifact_path`]: this exists to match a foreign
/// spelling — a path a Windows agent recorded, read back on any host — rather
/// than to preserve the local one.
pub fn normalize_separators(value: &str) -> String {
    value.replace('\\', "/")
}

/// Write `value` to `path` as pretty JSON with a two-space indent and a
/// trailing newline — the stable on-disk format for every artifact this binary
/// writes. `serde_json`'s `preserve_order` feature keeps object key order
/// stable so artifacts diff cleanly across runs.
///
/// A serialization failure maps to [`io::ErrorKind::InvalidData`]. In practice
/// it cannot happen: every caller serializes a `#[derive(Serialize)]` struct or
/// a `serde_json::Value`, neither of which can fail to render into a `String`.
pub fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    text.push('\n');
    fs::write(path, text)
}

/// Copy `source` to `destination`, recursing into directories and **preserving**
/// symlinks as symlinks. Missing parent directories of `destination` are created.
///
/// Use this only when the copy must round-trip faithfully; see
/// [`copy_entry_materialized`] for the content-freezing counterpart, which is
/// what callers copying into a task env or a snapshot want.
pub fn copy_entry(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        create_parent(destination)?;
        let target = fs::read_link(source)?;
        let to_directory = source.metadata().is_ok_and(|metadata| metadata.is_dir());
        create_symlink(&target, destination, to_directory)?;
    } else if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        create_parent(destination)?;
        fs::copy(source, destination)?;
    }
    Ok(())
}

/// Copy `source` to `destination`, recursing into directories and **resolving**
/// symlinks into their target's content.
///
/// The counterpart to [`copy_entry`], for callers that must freeze content
/// rather than mirror structure: a snapshot exists to be compared against
/// later, so a preserved link would silently track whatever it points at
/// instead of what was captured. Prefer [`copy_entry`] unless you specifically
/// need that guarantee.
pub fn copy_entry_materialized(source: &Path, destination: &Path) -> io::Result<()> {
    // `metadata` (unlike `symlink_metadata`) follows links, so a symlinked
    // directory recurses and a symlinked file lands in the `fs::copy` arm.
    if fs::metadata(source)?.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry_materialized(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        create_parent(destination)?;
        fs::copy(source, destination)?;
    }
    Ok(())
}

/// Create a symlink at `link` pointing at `target`.
///
/// `to_directory` is consulted only on Windows, which has separate file and
/// directory link kinds; POSIX has one. Creating a symlink there also needs
/// either Developer Mode or elevation, so this can fail for reasons that have
/// nothing to do with the paths involved.
pub(crate) fn create_symlink(target: &Path, link: &Path, to_directory: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        let _ = to_directory;
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    if to_directory {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

/// Create `path`'s parent directory chain, when it has one.
fn create_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent),
        None => Ok(()),
    }
}

/// Make `link` a second name for the directory `target`, for tests that need one
/// directory reachable by two spellings.
///
/// Deliberately *not* gated on the symlink capability. The paths that resolve
/// aliases differently are a Windows problem, so a fixture that skips on a
/// stock Windows box would leave that platform uncovered exactly where it
/// matters. A junction is the Windows alias that needs no Developer Mode and no
/// elevation, and `canonicalize` collapses it the same way it collapses a
/// symlink, an 8.3 short name, or a `subst` drive.
#[cfg(test)]
pub(crate) fn create_directory_alias(target: &Path, link: &Path) -> io::Result<()> {
    if !cfg!(windows) {
        return create_symlink(target, link, true);
    }
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "mklink /J could not alias {} to {}",
        link.display(),
        target.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Whether this host lets the test process create a symlink at all.
    ///
    /// A capability, not a platform: Windows can create symlinks, but only under
    /// Developer Mode or elevation. Probing beats gating on the OS — the tests
    /// then run wherever the capability exists instead of wherever the OS name
    /// matches.
    fn symlinks_available(scratch: &Path) -> bool {
        let target = scratch.join("probe-target.txt");
        let link = scratch.join("probe-link.txt");
        if fs::write(&target, "probe").is_err() {
            return false;
        }
        create_symlink(&target, &link, false).is_ok()
    }

    /// Report a skipped symlink test, deferring to the shared skip policy so the
    /// enforcement switch is decided in exactly one place.
    fn skip_without_symlinks(scratch: &Path, test: &str) -> bool {
        !symlinks_available(scratch)
            && crate::core::runtime::report_skip(
                test,
                "this host does not permit symlink creation (Windows needs Developer Mode)",
            )
    }

    /// Two names for one directory have to come back as one path — that is the
    /// entire point of the function.
    #[test]
    fn real_path_collapses_an_alias_onto_the_resolved_spelling() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real-dir");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(real.join("nested")).unwrap();
        let alias = tmp.path().join("alias-dir");
        create_directory_alias(&real, &alias).unwrap();

        assert_eq!(
            real_path(&alias.join("nested")).unwrap(),
            real_path(&real.join("nested")).unwrap()
        );
    }

    /// The verbatim prefix is an OS escape hatch: a spawned child reports the
    /// plain form, so a root carrying `\\?\` would fail to match every path the
    /// comparisons actually see.
    #[test]
    fn real_path_never_returns_a_verbatim_prefix() {
        let tmp = TempDir::new().unwrap();
        let resolved = real_path(tmp.path()).unwrap();
        assert!(
            !resolved.to_string_lossy().starts_with(r"\\?\"),
            "{resolved:?} still carries the verbatim prefix"
        );
    }

    /// A run names directories before it creates them — a workspace root, an
    /// iteration dir. Resolving only whole existing paths would leave exactly
    /// those unresolved, and the alias lives in the *ancestor* anyway (a temp
    /// dir, a junction, an 8.3 profile name), never in the leaf about to be
    /// created. So resolve as far down as the disk goes and re-attach the rest.
    #[test]
    fn real_path_resolves_the_existing_ancestor_of_a_path_not_yet_created() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real-dir");
        fs::create_dir_all(&real).unwrap();
        let alias = tmp.path().join("alias-dir");
        create_directory_alias(&real, &alias).unwrap();

        let unborn = alias.join("workspace").join("iteration-1");
        assert_eq!(
            real_path(&unborn).unwrap(),
            real_path(&real)
                .unwrap()
                .join("workspace")
                .join("iteration-1")
        );
    }

    /// With nothing on disk to anchor to, the lexical form is all there is —
    /// no worse than the spelling the caller passed in.
    #[test]
    fn real_path_falls_back_to_the_lexical_form_when_no_ancestor_exists() {
        let absent = Path::new(if cfg!(windows) {
            r"C:\no-such-root-here\child"
        } else {
            "/no-such-root-here/child"
        });
        assert_eq!(
            real_path(absent).unwrap(),
            std::path::absolute(absent).unwrap()
        );
    }

    /// A path already in wire form passes through unchanged on every host —
    /// the fixtures and goldens that spell paths POSIX-style stay byte-stable.
    #[test]
    fn artifact_path_leaves_a_forward_slash_path_alone() {
        assert_eq!(
            artifact_path(Path::new("/work/cond/run.json")),
            "/work/cond/run.json"
        );
    }

    /// A backslash means different things per host, so `artifact_path` does too,
    /// and both halves belong in one place.
    ///
    /// On Windows it is a separator: `Path::join` on a POSIX-rooted base emits
    /// one, so a manifest entry would otherwise read `/work/cond\run.json`. A
    /// verbatim `\\?\` prefix is stripped as well — an OS-level escape hatch, not
    /// something an agent should ever be handed. On POSIX a backslash is a legal
    /// filename character, so rewriting it would name a different file.
    #[test]
    fn artifact_path_applies_host_separator_rules() {
        if cfg!(windows) {
            assert_eq!(
                artifact_path(Path::new(r"/work/cond\run.json")),
                "/work/cond/run.json"
            );
            assert_eq!(
                artifact_path(Path::new(r"C:\work\cond\run.json")),
                "C:/work/cond/run.json"
            );
            assert_eq!(
                artifact_path(Path::new(r"\\?\C:\work\run.json")),
                "C:/work/run.json"
            );
            assert_eq!(
                artifact_path(Path::new(r"\\?\UNC\host\share\run.json")),
                "//host/share/run.json"
            );
        } else {
            assert_eq!(
                artifact_path(Path::new(r"/work/od\dity.json")),
                r"/work/od\dity.json"
            );
        }
    }

    /// Comparison normalization is unconditional, unlike [`artifact_path`]: its
    /// job is matching a *foreign* spelling — a Windows-recorded transcript read
    /// on any host — rather than preserving the local one.
    #[test]
    fn normalize_separators_rewrites_backslashes_on_every_host() {
        assert_eq!(
            normalize_separators(r"C:\work\cond\run.json"),
            "C:/work/cond/run.json"
        );
        assert_eq!(
            normalize_separators("/work/cond/run.json"),
            "/work/cond/run.json"
        );
    }

    /// The on-disk format is a contract: artifacts are diffed across runs and
    /// read by agents, so indent and the trailing newline are pinned.
    #[test]
    fn write_json_emits_two_space_indent_and_a_trailing_newline() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("artifact.json");

        write_json(&path, &json!({"b": 1, "a": [2]})).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\n  \"b\": 1,\n  \"a\": [\n    2\n  ]\n}\n"
        );
    }

    /// `preserve_order` is on, so keys serialize in insertion order rather than
    /// sorted — artifacts stay byte-stable across runs.
    #[test]
    fn write_json_preserves_key_insertion_order() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("artifact.json");

        write_json(&path, &json!({"zebra": 1, "apple": 2})).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.find("zebra").unwrap() < text.find("apple").unwrap(),
            "keys serialize in insertion order: {text}"
        );
    }

    #[test]
    fn copy_entry_copies_a_single_file() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src.txt");
        fs::write(&source, "payload").unwrap();

        copy_entry(&source, &tmp.path().join("dst.txt")).unwrap();

        assert_eq!(
            fs::read_to_string(tmp.path().join("dst.txt")).unwrap(),
            "payload"
        );
    }

    #[test]
    fn copy_entry_recurses_into_directories() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("tree");
        fs::create_dir_all(source.join("nested/deeper")).unwrap();
        fs::write(source.join("top.txt"), "top").unwrap();
        fs::write(source.join("nested/deeper/leaf.txt"), "leaf").unwrap();

        let destination = tmp.path().join("copied");
        copy_entry(&source, &destination).unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("top.txt")).unwrap(),
            "top"
        );
        assert_eq!(
            fs::read_to_string(destination.join("nested/deeper/leaf.txt")).unwrap(),
            "leaf"
        );
    }

    /// The destination's parent may not exist yet (staging writes into a tree it
    /// is still building). Failing here would make the helper's usability depend
    /// on caller ordering.
    #[test]
    fn copy_entry_creates_missing_destination_parents() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("src.txt");
        fs::write(&source, "payload").unwrap();

        let destination = tmp.path().join("a/b/c/dst.txt");
        copy_entry(&source, &destination).unwrap();

        assert_eq!(fs::read_to_string(&destination).unwrap(), "payload");
    }

    /// The behavior that used to differ between the five copies: a symlink must
    /// be recreated as a link, not resolved into its target's content. Following
    /// it would inline whatever the link pointed at — possibly from outside the
    /// tree being copied.
    #[test]
    fn copy_entry_recreates_symlinks_instead_of_following_them() {
        let tmp = TempDir::new().unwrap();
        if skip_without_symlinks(
            tmp.path(),
            "copy_entry_recreates_symlinks_instead_of_following_them",
        ) {
            return;
        }
        let target = tmp.path().join("target.txt");
        fs::write(&target, "target contents").unwrap();
        let link = tmp.path().join("link.txt");
        create_symlink(&target, &link, false).unwrap();

        let destination = tmp.path().join("copied-link.txt");
        copy_entry(&link, &destination).unwrap();

        assert!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the copy is still a symlink, not a materialized file"
        );
        assert_eq!(fs::read_link(&destination).unwrap(), target);
    }

    /// A symlink nested inside a copied directory survives too — the recursion
    /// arm must route back through the symlink arm, not through `fs::copy`.
    #[test]
    fn copy_entry_preserves_symlinks_nested_inside_a_directory() {
        let tmp = TempDir::new().unwrap();
        if skip_without_symlinks(
            tmp.path(),
            "copy_entry_preserves_symlinks_nested_inside_a_directory",
        ) {
            return;
        }
        let source = tmp.path().join("tree");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("real.txt"), "real").unwrap();
        create_symlink(Path::new("real.txt"), &source.join("alias.txt"), false).unwrap();

        let destination = tmp.path().join("copied");
        copy_entry(&source, &destination).unwrap();

        assert!(
            fs::symlink_metadata(destination.join("alias.txt"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the nested symlink is still a symlink"
        );
        assert_eq!(
            fs::read_link(destination.join("alias.txt")).unwrap(),
            Path::new("real.txt"),
            "the link target is preserved verbatim, including its relativeness"
        );
    }

    /// The counterpart semantic: a snapshot must freeze content, so a symlink is
    /// resolved and its target's bytes are written. Preserving the link would
    /// make the "frozen" copy track whatever the link points at later.
    #[test]
    fn copy_entry_materialized_resolves_symlinks_into_their_content() {
        let tmp = TempDir::new().unwrap();
        if skip_without_symlinks(
            tmp.path(),
            "copy_entry_materialized_resolves_symlinks_into_their_content",
        ) {
            return;
        }
        let source = tmp.path().join("tree");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("real.txt"), "frozen").unwrap();
        create_symlink(Path::new("real.txt"), &source.join("alias.txt"), false).unwrap();

        let destination = tmp.path().join("copied");
        copy_entry_materialized(&source, &destination).unwrap();

        let copied_alias = destination.join("alias.txt");
        assert!(
            !fs::symlink_metadata(&copied_alias)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the copy is a regular file, not a link"
        );
        assert_eq!(fs::read_to_string(&copied_alias).unwrap(), "frozen");

        // Mutating the original target must not change the frozen copy.
        fs::write(source.join("real.txt"), "changed").unwrap();
        assert_eq!(fs::read_to_string(&copied_alias).unwrap(), "frozen");
    }

    #[test]
    fn copy_entry_materialized_recurses_into_directories() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("tree");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested/leaf.txt"), "leaf").unwrap();

        let destination = tmp.path().join("copied");
        copy_entry_materialized(&source, &destination).unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("nested/leaf.txt")).unwrap(),
            "leaf"
        );
    }

    #[test]
    fn copy_entry_reports_a_missing_source() {
        let tmp = TempDir::new().unwrap();

        let err = copy_entry(&tmp.path().join("absent"), &tmp.path().join("dst")).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
