//! Framework-ignore entries written into a task environment.
//!
//! Staged skills live *inside* the task repository, under the harness's skills
//! dir. A codebase whose lint or format step globs the whole tree therefore
//! reports the framework's own artifacts as project failures — and only in the
//! arm that stages a skill, which biases the comparison and hands the agent
//! under test a red check it can neither fix nor understand (issue #296).
//!
//! The remedy is the one `.eval-magic-outputs/` already gets from
//! `.git/info/exclude`, extended to the tools that do not read Git's exclude
//! file: write the framework's own paths into the project's ignore files. Which
//! ignore files those are is detected from the codebase's tooling through the
//! packaged profiles below, or declared outright as `codebase.ignore_files`.
//!
//! `.gitignore` is deliberately never a target. The task-repository baseline
//! force-adds harness config dirs, so an entry there would hide nothing from
//! Git — but it *would* hide the staged skills from every `.gitignore`-aware
//! tool the agent uses, which breaks the treatment arm instead of fixing it.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::core::tree_profiles::{TreeProfile, detect};

include!(concat!(env!("OUT_DIR"), "/ignore_profiles.rs"));

/// Opening line of the block this module owns inside an ignore file.
const BLOCK_START: &str = "# >>> eval-magic framework files >>>";
/// Closing line of the block this module owns inside an ignore file.
const BLOCK_END: &str = "# <<< eval-magic framework files <<<";

/// One packaged tool profile: how to recognize the tool, and which ignore file
/// it reads.
#[derive(Debug, Deserialize)]
struct IgnoreProfile {
    id: String,
    /// Env-root-relative ignore file this tool honors.
    ignore_file: String,
    /// Whether the file may be created when the project does not have one.
    /// False where creating it would be inert or misleading — ESLint 9's flat
    /// config no longer reads `.eslintignore` at all.
    #[serde(default)]
    create_if_missing: bool,
    #[serde(default)]
    markers: Vec<String>,
    #[serde(default)]
    marker_patterns: Vec<String>,
    #[serde(default)]
    package_json_dependencies: Vec<String>,
}

impl TreeProfile for IgnoreProfile {
    fn id(&self) -> &str {
        &self.id
    }
    fn markers(&self) -> &[String] {
        &self.markers
    }
    fn marker_patterns(&self) -> &[String] {
        &self.marker_patterns
    }
    fn package_json_dependencies(&self) -> &[String] {
        &self.package_json_dependencies
    }
}

static PROFILES: LazyLock<Vec<IgnoreProfile>> = LazyLock::new(|| {
    let mut profiles: Vec<IgnoreProfile> = Vec::new();
    for (path, body) in PACKAGED_IGNORE_PROFILES {
        let profile: IgnoreProfile = toml::from_str(body)
            .unwrap_or_else(|error| panic!("invalid ignore profile {path}: {error}"));
        assert!(
            !profiles.iter().any(|existing| existing.id == profile.id),
            "duplicate ignore profile {}",
            profile.id
        );
        profiles.push(profile);
    }
    profiles
});

/// What to write into one task environment.
pub struct IgnorePlan<'a> {
    /// The codebase's `ignore_files` declaration. `Some` replaces detection
    /// outright — an empty slice opts out of the whole mechanism.
    pub declared: Option<&'a [String]>,
    /// Env-relative paths the runner itself places, in the order they should
    /// appear in the block.
    pub framework_paths: &'a [String],
}

/// What [`apply_framework_ignore_entries`] did.
#[derive(Debug, Default)]
pub struct IgnoreOutcome {
    /// Env-relative ignore files actually written, sorted.
    pub written: Vec<String>,
    /// Warnings for the CLI to print; the library never prints.
    pub warnings: Vec<String>,
}

/// Write the framework block into every ignore file this environment needs.
///
/// Idempotent: a second call rewrites the block in place rather than appending
/// a second one, so an environment rebuilt over an existing tree stays clean.
pub fn apply_framework_ignore_entries(
    env_root: &Path,
    plan: &IgnorePlan,
) -> io::Result<IgnoreOutcome> {
    let mut outcome = IgnoreOutcome::default();
    let block = render_block(plan.framework_paths);
    for (relative, create_if_missing) in targets(env_root, plan)? {
        let path = relative
            .split('/')
            .fold(env_root.to_path_buf(), |path, segment| path.join(segment));
        let existing = match fs::symlink_metadata(&path) {
            Ok(_) => match existing_ignore_file(env_root, &path)? {
                Ok(content) => Some(content),
                Err(reason) => {
                    outcome.warnings.push(format!(
                        "{relative} {reason}; eval-magic could not hide its own staged files from \
                         this project's tooling there"
                    ));
                    continue;
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if existing.is_none() && !create_if_missing {
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &path,
            merge_block(existing.as_deref().unwrap_or_default(), &block),
        )?;
        outcome.written.push(relative);
    }
    outcome.written.sort();
    outcome.written.dedup();
    Ok(outcome)
}

/// The ignore file already at `path`, or why it cannot be used.
///
/// A symlink is followed — a monorepo may legitimately point one ignore file at
/// another — but only after its target is proven to stay inside the task
/// environment, so nothing here can write onto the host through a link the
/// sourced codebase carried in.
fn existing_ignore_file(env_root: &Path, path: &Path) -> io::Result<Result<String, &'static str>> {
    let Ok(resolved) = fs::canonicalize(path) else {
        return Ok(Err("does not resolve to a readable file"));
    };
    if !resolved.starts_with(fs::canonicalize(env_root)?) {
        return Ok(Err("resolves outside the task environment"));
    }
    if !fs::metadata(&resolved)?.is_file() {
        return Ok(Err("is not a regular file"));
    }
    Ok(Ok(fs::read_to_string(&resolved)?))
}

/// The ignore files to write, as `(env-relative path, may create)`.
///
/// A declaration replaces detection entirely, and a declared path is always
/// created: the author named it, so its absence is not a signal.
fn targets(env_root: &Path, plan: &IgnorePlan) -> io::Result<Vec<(String, bool)>> {
    if let Some(declared) = plan.declared {
        return Ok(declared.iter().map(|path| (path.clone(), true)).collect());
    }
    let detected = detect(
        env_root,
        PROFILES.iter().map(|profile| profile as &dyn TreeProfile),
    )?;
    Ok(PROFILES
        .iter()
        .filter(|profile| detected.iter().any(|id| id == &profile.id))
        .map(|profile| (profile.ignore_file.clone(), profile.create_if_missing))
        .collect())
}

/// The block as it appears in every target file, gitignore pattern syntax —
/// which Prettier, ESLint, Stylelint, markdownlint, and Docker all accept.
fn render_block(framework_paths: &[String]) -> String {
    let mut block = format!(
        "{BLOCK_START}\n\
         # Staged by `eval-magic run` so this project's own tooling does not report them.\n\
         # See `eval-magic docs codebase`.\n"
    );
    for path in framework_paths {
        block.push_str(path);
        block.push('\n');
    }
    block.push_str(BLOCK_END);
    block.push('\n');
    block
}

/// `existing` with the framework block replaced in place, or appended after a
/// normalizing final newline when it is not there yet.
fn merge_block(existing: &str, block: &str) -> String {
    if let Some(start) = existing.find(BLOCK_START)
        && let Some(end) = existing[start..].find(BLOCK_END)
    {
        let after = start + end + BLOCK_END.len();
        let tail = existing[after..]
            .strip_prefix('\n')
            .unwrap_or(&existing[after..]);
        return format!("{}{block}{tail}", &existing[..start]);
    }
    if existing.is_empty() {
        return block.to_string();
    }
    let separator = if existing.ends_with('\n') { "" } else { "\n" };
    format!("{existing}{separator}{block}")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::{IgnorePlan, apply_framework_ignore_entries};
    use crate::core::fs::create_symlink;

    /// Whether this host lets the test process create a symlink at all — a
    /// capability, not a platform label. Mirrors the probe in `core::fs`.
    fn skip_without_symlinks(scratch: &Path, test: &str) -> bool {
        let target = scratch.join("probe-target.txt");
        let link = scratch.join("probe-link.txt");
        let available =
            fs::write(&target, "probe").is_ok() && create_symlink(&target, &link).is_ok();
        let _ = fs::remove_file(&link);
        let _ = fs::remove_file(&target);
        !available
            && crate::core::runtime::report_skip(
                test,
                "this filesystem does not permit symlink creation",
            )
    }

    const FRAMEWORK: [&str; 3] = [
        "/.eval-magic-outputs/",
        "/.claude/skills/",
        "/.claude/settings.local.json",
    ];

    fn apply(root: &Path, declared: Option<&[String]>) -> super::IgnoreOutcome {
        let framework_paths: Vec<String> = FRAMEWORK.iter().map(|path| path.to_string()).collect();
        apply_framework_ignore_entries(
            root,
            &IgnorePlan {
                declared,
                framework_paths: &framework_paths,
            },
        )
        .unwrap()
    }

    /// The packaged set is data, and a broken entry would fail silently — a
    /// profile nothing can detect, or two profiles fighting over one file.
    #[test]
    fn every_packaged_profile_is_detectable_and_owns_one_contained_ignore_file() {
        let mut seen: Vec<&str> = Vec::new();
        for profile in super::PROFILES.iter() {
            let id = &profile.id;
            assert!(
                !profile.markers.is_empty()
                    || !profile.marker_patterns.is_empty()
                    || !profile.package_json_dependencies.is_empty(),
                "profile {id} declares no way to detect it"
            );
            assert!(
                !profile.ignore_file.starts_with('/')
                    && !profile.ignore_file.split('/').any(|part| part == ".."),
                "profile {id} names an ignore file outside the task environment: {}",
                profile.ignore_file
            );
            assert!(
                !seen.contains(&profile.ignore_file.as_str()),
                "profile {id} claims {}, which another profile already owns",
                profile.ignore_file
            );
            seen.push(&profile.ignore_file);
        }
        assert!(!seen.is_empty(), "no ignore profiles were packaged");
    }

    #[test]
    fn a_detected_formatter_gets_its_ignore_file_created_with_every_framework_path() {
        let root = tempdir().unwrap();
        fs::write(root.path().join(".prettierrc.json"), "{}").unwrap();

        let outcome = apply(root.path(), None);

        assert_eq!(outcome.written, [".prettierignore"]);
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
        let body = fs::read_to_string(root.path().join(".prettierignore")).unwrap();
        for path in FRAMEWORK {
            assert!(body.contains(path), "missing {path} in:\n{body}");
        }
    }

    #[test]
    fn a_package_json_dependency_detects_the_tool_too() {
        let root = tempdir().unwrap();
        fs::write(
            root.path().join("package.json"),
            r#"{"devDependencies":{"prettier":"3.0.0"}}"#,
        )
        .unwrap();

        let outcome = apply(root.path(), None);

        assert_eq!(outcome.written, [".prettierignore"]);
    }

    #[test]
    fn rewriting_replaces_the_block_instead_of_appending_a_second_one() {
        let root = tempdir().unwrap();
        fs::write(root.path().join(".prettierrc.json"), "{}").unwrap();

        apply(root.path(), None);
        let first = fs::read_to_string(root.path().join(".prettierignore")).unwrap();
        apply(root.path(), None);
        let second = fs::read_to_string(root.path().join(".prettierignore")).unwrap();

        assert_eq!(first, second);
        assert_eq!(second.matches("/.claude/skills/").count(), 1);
    }

    #[test]
    fn the_projects_own_entries_survive_and_a_missing_final_newline_is_repaired() {
        let root = tempdir().unwrap();
        fs::write(root.path().join(".prettierrc.json"), "{}").unwrap();
        fs::write(root.path().join(".prettierignore"), "dist\ncoverage").unwrap();

        apply(root.path(), None);

        let body = fs::read_to_string(root.path().join(".prettierignore")).unwrap();
        assert!(body.starts_with("dist\ncoverage\n"), "clobbered:\n{body}");
        assert!(body.contains("/.claude/skills/"));
    }

    #[test]
    fn eslints_ignore_file_is_appended_to_but_never_created() {
        let root = tempdir().unwrap();
        fs::write(root.path().join(".eslintrc.json"), "{}").unwrap();

        let outcome = apply(root.path(), None);

        assert!(outcome.written.is_empty(), "{:?}", outcome.written);
        assert!(!root.path().join(".eslintignore").exists());

        fs::write(root.path().join(".eslintignore"), "vendor\n").unwrap();
        let outcome = apply(root.path(), None);

        assert_eq!(outcome.written, [".eslintignore"]);
        let body = fs::read_to_string(root.path().join(".eslintignore")).unwrap();
        assert!(body.starts_with("vendor\n"));
        assert!(body.contains("/.claude/skills/"));
    }

    #[test]
    fn a_declared_list_replaces_detection_and_creates_parent_directories() {
        let root = tempdir().unwrap();
        fs::write(root.path().join(".prettierrc.json"), "{}").unwrap();
        let declared = ["config/.prettierignore".to_string()];

        let outcome = apply(root.path(), Some(&declared));

        assert_eq!(outcome.written, ["config/.prettierignore"]);
        assert!(!root.path().join(".prettierignore").exists());
        let body = fs::read_to_string(root.path().join("config/.prettierignore")).unwrap();
        assert!(body.contains("/.claude/skills/"));
    }

    #[test]
    fn an_empty_declared_list_opts_out_entirely() {
        let root = tempdir().unwrap();
        fs::write(root.path().join(".prettierrc.json"), "{}").unwrap();

        let outcome = apply(root.path(), Some(&[]));

        assert!(outcome.written.is_empty(), "{:?}", outcome.written);
        assert!(!root.path().join(".prettierignore").exists());
    }

    #[test]
    fn a_directory_where_an_ignore_file_belongs_is_reported_not_written() {
        let root = tempdir().unwrap();
        fs::write(root.path().join(".prettierrc.json"), "{}").unwrap();
        fs::create_dir(root.path().join(".prettierignore")).unwrap();

        let outcome = apply(root.path(), None);

        assert!(outcome.written.is_empty(), "{:?}", outcome.written);
        assert_eq!(outcome.warnings.len(), 1, "{:?}", outcome.warnings);
        assert!(
            outcome.warnings[0].contains(".prettierignore"),
            "{}",
            outcome.warnings[0]
        );
    }

    #[test]
    fn a_symlinked_ignore_file_inside_the_environment_is_written_through() {
        let root = tempdir().unwrap();
        if skip_without_symlinks(
            root.path(),
            "a_symlinked_ignore_file_inside_the_environment_is_written_through",
        ) {
            return;
        }
        fs::write(root.path().join(".prettierrc.json"), "{}").unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        fs::write(root.path().join("config/ignore-rules"), "dist\n").unwrap();
        create_symlink(
            Path::new("config/ignore-rules"),
            &root.path().join(".prettierignore"),
        )
        .unwrap();

        let outcome = apply(root.path(), None);

        assert_eq!(outcome.written, [".prettierignore"]);
        let body = fs::read_to_string(root.path().join("config/ignore-rules")).unwrap();
        assert!(body.starts_with("dist\n"), "clobbered:\n{body}");
        assert!(body.contains("/.claude/skills/"));
    }

    #[test]
    fn a_symlink_escaping_the_environment_is_reported_not_followed() {
        let outside = tempdir().unwrap();
        let root = tempdir().unwrap();
        if skip_without_symlinks(
            root.path(),
            "a_symlink_escaping_the_environment_is_reported_not_followed",
        ) {
            return;
        }
        fs::write(outside.path().join("host.prettierignore"), "dist\n").unwrap();
        fs::write(root.path().join(".prettierrc.json"), "{}").unwrap();
        create_symlink(
            &outside.path().join("host.prettierignore"),
            &root.path().join(".prettierignore"),
        )
        .unwrap();

        let outcome = apply(root.path(), None);

        assert!(outcome.written.is_empty(), "{:?}", outcome.written);
        assert_eq!(outcome.warnings.len(), 1, "{:?}", outcome.warnings);
        assert_eq!(
            fs::read_to_string(outside.path().join("host.prettierignore")).unwrap(),
            "dist\n",
            "wrote outside the task environment"
        );
    }

    #[test]
    fn a_codebase_with_no_detected_tooling_is_left_alone() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("Cargo.toml"), "").unwrap();

        let outcome = apply(root.path(), None);

        assert!(outcome.written.is_empty(), "{:?}", outcome.written);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
