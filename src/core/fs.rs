//! Shared filesystem + JSON helpers — the single home for artifact writing and
//! tree copying, used by `pipeline`, `workspace`, `cli::run`, and `sandbox`.
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
use std::path::Path;

use serde::Serialize;

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
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, destination)?;
        #[cfg(windows)]
        if source.metadata().is_ok_and(|metadata| metadata.is_dir()) {
            std::os::windows::fs::symlink_dir(target, destination)?;
        } else {
            std::os::windows::fs::symlink_file(target, destination)?;
        }
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
    #[cfg(unix)]
    #[test]
    fn copy_entry_recreates_symlinks_instead_of_following_them() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("target.txt");
        fs::write(&target, "target contents").unwrap();
        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

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
    #[cfg(unix)]
    #[test]
    fn copy_entry_preserves_symlinks_nested_inside_a_directory() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("tree");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("real.txt"), "real").unwrap();
        std::os::unix::fs::symlink("real.txt", source.join("alias.txt")).unwrap();

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
    #[cfg(unix)]
    #[test]
    fn copy_entry_materialized_resolves_symlinks_into_their_content() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("tree");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("real.txt"), "frozen").unwrap();
        std::os::unix::fs::symlink("real.txt", source.join("alias.txt")).unwrap();

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
