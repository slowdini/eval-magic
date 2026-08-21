//! Copy an eval's fixtures into its isolated task env, laid out like a real repo
//! so the agent-under-test reads them at natural project-relative paths.
//! [`FixtureClaims`] retains the legacy shared-group collision checks used by the
//! environment planner.

use std::fs;
use std::path::Path;

use crate::core::{AssertionCommandCheck, Eval};

use super::RunError;
use crate::core::fs::copy_entry_materialized;

/// Cross-eval claims on env-relative fixture destinations: `dest → (eval_id, source)`.
/// Used when a planned group contains multiple evals; canonical diff-scope runs
/// currently task-scope groups, but legacy plans remain readable.
pub type FixtureClaims = std::collections::HashMap<String, (String, String)>;

/// Record that `eval_id` provides the fixture at env-relative `dest` from `source`.
/// Returns `Ok(true)` when the dest was already claimed from the same source (idempotent
/// share — skip the re-copy), `Ok(false)` on the first claim, and `Err` when a *different*
/// source already claimed the same dest (an order-dependent cross-eval clobber).
fn claim_fixture_dest(
    claims: &mut FixtureClaims,
    eval_id: &str,
    dest: &str,
    source: &str,
) -> Result<bool, RunError> {
    if let Some((prev_eval, prev_source)) = claims.get(dest) {
        if prev_source != source {
            return Err(RunError::msg(format!(
                "fixture conflict: evals '{prev_eval}' and '{eval_id}' both place a fixture at env path '{dest}' from different sources ('{prev_source}' vs '{source}'). Give them distinct paths."
            )));
        }
        return Ok(true);
    }
    claims.insert(dest.to_string(), (eval_id.to_string(), source.to_string()));
    Ok(false)
}

/// True for a path that is absolute under *either* platform's rules.
///
/// `Path::is_absolute` answers for the host only: Windows has no root without a
/// drive, so `/etc/passwd` reads as relative there and slips past a bare
/// `is_absolute` check — then `Path::join`'s root-replacing behavior lands it
/// outside the env entirely. Eval configs are committed and run on every
/// platform, so the verdict must not depend on which host reads them.
///
/// A drive-relative Windows path (`C:fixture.txt`) is only detectable where the
/// parser produces a `Prefix` component, so it is caught on Windows and treated
/// as an ordinary relative name elsewhere.
fn is_absolute_on_any_platform(raw: &str) -> bool {
    let path = Path::new(raw);
    path.has_root()
        || raw.starts_with('\\')
        || matches!(
            path.components().next(),
            Some(std::path::Component::Prefix(_))
        )
}

/// Reject a fixture path that is absolute or escapes `env/` via `..`, so a fixture
/// always lands inside the isolated env.
fn validate_fixture_rel(f: &str) -> Result<(), RunError> {
    let p = Path::new(f);
    let escapes = is_absolute_on_any_platform(f)
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
    if escapes {
        return Err(RunError::msg(format!(
            "fixture path must be relative and stay within env: {f}"
        )));
    }
    let first_normal = p.components().find_map(|component| match component {
        std::path::Component::Normal(value) => Some(value.to_string_lossy()),
        _ => None,
    });
    if first_normal.is_some_and(|component| component.eq_ignore_ascii_case(".git")) {
        return Err(RunError::msg(format!(
            "fixture path uses reserved runner-owned Git metadata at task root: {f}"
        )));
    }
    Ok(())
}

/// Reject a fixture source root that is absolute or escapes `<skill>/evals/`.
/// Unlike a fixture destination, this path never lands in the task repository,
/// so a leading `.git` component is not runner-owned metadata.
fn validate_files_root_rel(root: &str) -> Result<(), RunError> {
    let path = Path::new(root);
    let escapes = is_absolute_on_any_platform(root)
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir));
    if escapes {
        return Err(RunError::msg(format!(
            "files_root must be relative and stay within the skill's evals directory: {root}"
        )));
    }
    Ok(())
}

/// Resolve an eval's declared fixtures to `(env-relative dest, source path)` pairs,
/// validating each path stays within the env and that the source exists — without
/// copying anything. [`super::grouping`] consumes these pairs to detect cross-eval
/// clobbers before any env is built, and [`copy_fixtures`] reuses them, so fixture
/// path resolution lives in exactly one place.
pub fn fixture_pairs(ev: &Eval, skill_dir: &Path) -> Result<Vec<(String, String)>, RunError> {
    let mut source_root = skill_dir.join("evals");
    if let Some(root) = ev.files_root.as_deref() {
        validate_files_root_rel(root)?;
        source_root = source_root.join(root);
    }
    let Some(files) = ev.files.as_ref().filter(|f| !f.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut pairs = Vec::with_capacity(files.len());
    for f in files {
        validate_fixture_rel(f)?;
        let src = source_root.join(f);
        if !src.exists() {
            return Err(RunError::msg(format!(
                "fixture not found: {}",
                src.display()
            )));
        }
        pairs.push((f.clone(), src.to_string_lossy().into_owned()));
    }
    Ok(pairs)
}

/// Resolve a command check's held-out setup paths without copying them. This is
/// called during read-only run resolution so a missing source fails before any
/// workspace or staged environment is created.
pub fn setup_file_pairs(
    check: &AssertionCommandCheck,
    skill_dir: &Path,
) -> Result<Vec<(String, String)>, RunError> {
    let Some(files) = check.setup_files.as_ref().filter(|files| !files.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut pairs = Vec::with_capacity(files.len());
    for file in files {
        validate_fixture_rel(file)?;
        let source = skill_dir.join("evals").join(file);
        if !source.exists() {
            return Err(RunError::msg(format!(
                "command-check setup file not found: {}",
                source.display()
            )));
        }
        pairs.push((file.clone(), source.to_string_lossy().into_owned()));
    }
    Ok(pairs)
}

/// Copy an eval's fixture files into `env_root`, preserving each declared relative path
/// so the env reads like a real repo (`files: ["src/main.rs"]` → `env/src/main.rs`), and
/// returning the env-relative paths (the agent-under-test's cwd is `env/`). Fixtures are
/// shared across conditions and runs within one env; `claims` dedups idempotent
/// re-declarations and rejects cross-eval clobbers. Cross-eval clobbers are routed into
/// separate isolation groups by [`super::grouping`] before this is called per group, so
/// within a single group's env a clobber should never reach the `claims` rejection.
pub fn copy_fixtures(
    ev: &Eval,
    skill_dir: &Path,
    env_root: &Path,
    claims: &mut FixtureClaims,
) -> Result<Vec<String>, RunError> {
    let pairs = fixture_pairs(ev, skill_dir)?;
    let mut copied = Vec::with_capacity(pairs.len());
    for (dest, source) in &pairs {
        let already = claim_fixture_dest(claims, &ev.id, dest, source)?;
        if !already {
            let dst = env_root.join(dest);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            copy_entry_materialized(Path::new(source), &dst)?;
        }
        copied.push(dest.clone());
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_with_files(id: &str, files: &[&str]) -> Eval {
        Eval {
            id: id.to_string(),
            prompt: "p".to_string(),
            expected_output: "o".to_string(),
            files: Some(files.iter().map(|f| (*f).to_string()).collect()),
            files_root: None,
            assertions: None,
            skill_should_trigger: None,
            runs: None,
            isolation: None,
            turns: None,
            codebase: None,
            responder: None,
        }
    }

    #[test]
    fn fixture_pairs_resolves_dest_and_source_without_copying() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill");
        let evals = skill_dir.join("evals");
        fs::create_dir_all(evals.join("data")).unwrap();
        fs::write(evals.join("config.json"), "cfg").unwrap();
        fs::write(evals.join("data/x.json"), "xx").unwrap();

        let ev = eval_with_files("e1", &["config.json", "data/x.json"]);
        let pairs = fixture_pairs(&ev, &skill_dir).unwrap();

        assert_eq!(
            pairs,
            vec![
                (
                    "config.json".to_string(),
                    evals.join("config.json").to_string_lossy().into_owned()
                ),
                (
                    "data/x.json".to_string(),
                    evals.join("data/x.json").to_string_lossy().into_owned()
                ),
            ]
        );
        // Pure: it resolves paths but copies nothing.
        assert!(!tmp.path().join("env").exists());
    }

    #[test]
    fn fixture_pairs_empty_when_no_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill");
        fs::create_dir_all(skill_dir.join("evals")).unwrap();
        let ev = eval_with_files("e1", &[]);
        assert!(fixture_pairs(&ev, &skill_dir).unwrap().is_empty());
    }

    #[test]
    fn fixture_pairs_rejects_escapes_and_missing_sources() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill");
        fs::create_dir_all(skill_dir.join("evals")).unwrap();

        let escaping = eval_with_files("e1", &["../escape.txt"]);
        assert!(
            fixture_pairs(&escaping, &skill_dir)
                .unwrap_err()
                .to_string()
                .contains("relative")
        );

        let missing = eval_with_files("e1", &["nope.json"]);
        assert!(
            fixture_pairs(&missing, &skill_dir)
                .unwrap_err()
                .to_string()
                .contains("fixture not found")
        );
    }

    #[test]
    fn fixture_pairs_rejects_files_root_escapes_even_when_sources_exist() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill");
        let evals = skill_dir.join("evals");
        fs::create_dir_all(&evals).unwrap();

        let outside = skill_dir.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret").unwrap();

        for root in [
            "../outside".to_string(),
            outside.to_string_lossy().into_owned(),
        ] {
            let mut ev = eval_with_files("e1", &["secret.txt"]);
            ev.files_root = Some(root);
            let error = fixture_pairs(&ev, &skill_dir).unwrap_err().to_string();
            assert!(error.contains("files_root"), "error was: {error}");
            assert!(error.contains("relative"), "error was: {error}");
        }
    }

    #[test]
    fn fixture_pairs_validates_files_root_without_declared_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill");
        fs::create_dir_all(skill_dir.join("evals")).unwrap();
        let mut ev = eval_with_files("e1", &[]);
        ev.files_root = Some("../outside".to_string());

        let error = fixture_pairs(&ev, &skill_dir).unwrap_err().to_string();

        assert!(error.contains("files_root"), "error was: {error}");
        assert!(error.contains("relative"), "error was: {error}");
    }

    #[test]
    fn fixture_paths_reserve_only_a_root_dot_git_component_case_insensitively() {
        for bad in [
            ".git",
            ".git/config",
            "./.GIT/config",
            ".GiT/hooks/pre-commit",
        ] {
            let error = validate_fixture_rel(bad).unwrap_err();
            assert!(
                error.to_string().contains("reserved"),
                "expected root Git metadata rejection for {bad}, got: {error}"
            );
        }

        validate_fixture_rel("vendor/.git/config").unwrap();
    }

    #[test]
    fn copy_fixtures_preserves_declared_relative_paths_in_env() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill");
        let evals = skill_dir.join("evals");
        fs::create_dir_all(evals.join("data")).unwrap();
        fs::write(evals.join("config.json"), "cfg").unwrap();
        fs::write(evals.join("data/x.json"), "xx").unwrap();
        let env_root = tmp.path().join("env");

        let ev = eval_with_files("e1", &["config.json", "data/x.json"]);
        let mut claims = FixtureClaims::new();
        let copied = copy_fixtures(&ev, &skill_dir, &env_root, &mut claims).unwrap();

        // Structure preserved under env/, not flattened into an inputs/ bucket.
        assert_eq!(
            fs::read_to_string(env_root.join("config.json")).unwrap(),
            "cfg"
        );
        assert_eq!(
            fs::read_to_string(env_root.join("data/x.json")).unwrap(),
            "xx"
        );
        assert!(!env_root.join("inputs").exists());
        // Returns env-relative declared paths (the agent's cwd is env).
        assert_eq!(
            copied,
            vec!["config.json".to_string(), "data/x.json".to_string()]
        );
    }

    #[test]
    fn copy_fixtures_rejects_parent_escaping_and_absolute_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skill");
        fs::create_dir_all(skill_dir.join("evals")).unwrap();
        let env_root = tmp.path().join("env");

        // `\etc\passwd` is absolute on Windows and a legal filename on Unix.
        // Eval configs are committed and run on every platform, so the verdict
        // must not depend on which host reads them — a fixture that validates
        // on Linux and escapes the env on Windows is the worst of both.
        for bad in [
            "../escape.txt",
            "/etc/passwd",
            "a/../../b.txt",
            r"\etc\passwd",
        ] {
            let ev = eval_with_files("e1", &[bad]);
            let mut claims = FixtureClaims::new();
            let err = copy_fixtures(&ev, &skill_dir, &env_root, &mut claims).unwrap_err();
            assert!(
                err.to_string().contains("relative"),
                "expected a path-traversal rejection for {bad}, got: {err}"
            );
        }
    }

    #[test]
    fn claim_fixture_dest_allows_idempotent_share_errors_on_different_source() {
        let mut claims = FixtureClaims::new();
        // First eval claims the dest.
        assert!(
            !claim_fixture_dest(&mut claims, "e1", "config.json", "/a/evals/config.json").unwrap()
        );
        // A second eval declaring the same dest from the same source is an idempotent share.
        assert!(
            claim_fixture_dest(&mut claims, "e2", "config.json", "/a/evals/config.json").unwrap()
        );
        // The same dest from a *different* source is an ambiguous cross-eval conflict.
        let err = claim_fixture_dest(&mut claims, "e3", "config.json", "/b/evals/config.json")
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("e1"), "names the first claimer: {msg}");
        assert!(msg.contains("e3"), "names the conflicting eval: {msg}");
        assert!(msg.contains("config.json"), "names the path: {msg}");
    }
}
