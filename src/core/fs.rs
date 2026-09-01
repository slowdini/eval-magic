//! Shared filesystem + JSON helpers — the single home for artifact writing,
//! artifact path rendering, and tree copying, used by `pipeline`, `workspace`,
//! `cli::run`, `adapters`, and `sandbox`.
//!
//! [`artifact_path`] renders a supported-host path into the forward-slash wire
//! format every generated artifact carries; [`normalize_separators`] is its
//! comparison-side counterpart, for matching foreign path spellings in data.
//!
//! [`copy_entry_materialized`] is the one way to copy here, and it resolves
//! symlinks into their target's content rather than mirroring them. Every
//! destination in this tree wants that: staging and overlays copy *into* an
//! isolated task env, where a preserved link would point back out of the
//! sandbox, and a snapshot must freeze content so a later run compares against
//! what was captured rather than whatever the link now points at.
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
/// join them, and the golden fixtures compare them byte for byte. Supported
/// hosts use forward slashes natively. A POSIX filename may legally contain a
/// literal backslash, so rewriting one would name a different file.
///
/// Not for paths handed to a process: a spawned command's argv and the guard
/// hook command line must keep the host's own spelling.
pub fn artifact_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// FNV-1a over `bytes`, as 16 hex characters.
///
/// Hand-rolled rather than `DefaultHasher`, which carries no stability
/// guarantee across Rust releases: digests that outlive a process (workspace
/// slugs, prep-time descriptor provenance) must not shift under a toolchain
/// upgrade.
pub fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The one spelling of `path` that every participant in a run agrees on.
///
/// `getcwd` and `canonicalize` resolve symlink aliases, so resolving once at the
/// point a run's roots are derived gives the write guard, Git, and spawned tools
/// one spelling to compare.
///
/// A run names directories before it creates them, so resolution walks up to the
/// deepest ancestor that exists and re-attaches the rest. With no ancestor on
/// disk at all, the lexical form is all there is.
pub fn real_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut unresolved = Vec::new();
    let mut anchor = absolute.as_path();
    loop {
        if let Ok(canonical) = fs::canonicalize(anchor) {
            let mut resolved = canonical;
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

/// Copy `source` to `destination`, recursing into directories and **resolving**
/// symlinks into their target's content.
///
/// Callers here must freeze content rather than mirror structure: a snapshot
/// exists to be compared against later, so a preserved link would silently
/// track whatever it points at instead of what was captured.
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

/// Whether a file created under `from` can be hard-linked into `to` — the
/// capability `git clone --local` relies on to share an object store instead
/// of copying it.
///
/// Two directories a run owns can sit on different filesystems (a workspace
/// on a mounted volume, a cache on tmpfs), and `link(2)` is what says so.
/// Any failure reads as unavailable, so the caller falls back to copying rather
/// than provisioning wrong.
pub fn hardlinks_available(from: &Path, to: &Path) -> bool {
    let Ok(probe) = tempfile::NamedTempFile::new_in(from) else {
        return false;
    };
    let Some(name) = probe.path().file_name() else {
        return false;
    };
    let target = to.join(name);
    match fs::hard_link(probe.path(), &target) {
        Ok(()) => {
            let _ = fs::remove_file(&target);
            true
        }
        Err(_) => false,
    }
}

/// Create a symlink at `link` pointing at `target`.
///
/// Test support. Copying here resolves links into content rather than
/// recreating them, so the only callers left are fixtures that need a link to
/// exist and the probe that asks whether this filesystem permits one.
#[cfg(test)]
pub(crate) fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Create `path`'s parent directory chain, when it has one.
fn create_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Whether this host lets the test process create a symlink at all.
    ///
    /// A capability, not a platform label: tests exercise links wherever the
    /// backing filesystem permits them.
    fn symlinks_available(scratch: &Path) -> bool {
        let target = scratch.join("probe-target.txt");
        let link = scratch.join("probe-link.txt");
        if fs::write(&target, "probe").is_err() {
            return false;
        }
        create_symlink(&target, &link).is_ok()
    }

    /// Report a skipped symlink test, deferring to the shared skip policy so the
    /// enforcement switch is decided in exactly one place.
    fn skip_without_symlinks(scratch: &Path, test: &str) -> bool {
        !symlinks_available(scratch)
            && crate::core::runtime::report_skip(
                test,
                "this filesystem does not permit symlink creation",
            )
    }

    /// Two names for one directory have to come back as one path — that is the
    /// entire point of the function.
    #[test]
    fn real_path_collapses_an_alias_onto_the_resolved_spelling() {
        let tmp = TempDir::new().unwrap();
        if skip_without_symlinks(
            tmp.path(),
            "real_path_collapses_an_alias_onto_the_resolved_spelling",
        ) {
            return;
        }
        let real = tmp.path().join("real-dir");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(real.join("nested")).unwrap();
        let alias = tmp.path().join("alias-dir");
        create_symlink(&real, &alias).unwrap();

        assert_eq!(
            real_path(&alias.join("nested")).unwrap(),
            real_path(&real.join("nested")).unwrap()
        );
    }

    /// A run names directories before it creates them — a workspace root, an
    /// iteration dir. Resolving only whole existing paths would leave exactly
    /// those unresolved, and the alias lives in the *ancestor* anyway, never in
    /// the leaf about to be created. Resolve as far down as the disk goes and
    /// re-attach the rest.
    #[test]
    fn real_path_resolves_the_existing_ancestor_of_a_path_not_yet_created() {
        let tmp = TempDir::new().unwrap();
        if skip_without_symlinks(
            tmp.path(),
            "real_path_resolves_the_existing_ancestor_of_a_path_not_yet_created",
        ) {
            return;
        }
        let real = tmp.path().join("real-dir");
        fs::create_dir_all(&real).unwrap();
        let alias = tmp.path().join("alias-dir");
        create_symlink(&real, &alias).unwrap();

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
        let absent = Path::new("/no-such-root-here/child");
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

    /// A backslash is a legal POSIX filename character, so artifact rendering
    /// preserves it rather than naming a different file.
    #[test]
    fn artifact_path_preserves_literal_backslashes() {
        assert_eq!(
            artifact_path(Path::new(r"/work/od\dity.json")),
            r"/work/od\dity.json"
        );
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
        create_symlink(Path::new("real.txt"), &source.join("alias.txt")).unwrap();

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

    /// The probe both succeeds and cleans up after itself: it runs inside the
    /// per-iteration codebase cache, where a leftover file would ship into the
    /// next environment built from it.
    #[test]
    fn hardlinks_available_is_true_between_directories_on_one_filesystem() {
        let tmp = TempDir::new().unwrap();
        let from = tmp.path().join("from");
        let to = tmp.path().join("to");
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();

        assert!(hardlinks_available(&from, &to));

        assert_eq!(
            fs::read_dir(&from).unwrap().count(),
            0,
            "no probe residue in from"
        );
        assert_eq!(
            fs::read_dir(&to).unwrap().count(),
            0,
            "no probe residue in to"
        );
    }

    /// Any failure — a missing directory on either side, a filesystem that
    /// refuses the link — reads as "unavailable", so callers fall back to
    /// copying rather than provisioning wrong.
    #[test]
    fn hardlinks_available_is_false_when_a_directory_is_missing() {
        let tmp = TempDir::new().unwrap();
        let present = tmp.path().join("present");
        fs::create_dir_all(&present).unwrap();

        assert!(!hardlinks_available(&tmp.path().join("absent"), &present));
        assert!(!hardlinks_available(&present, &tmp.path().join("absent")));
    }
}
