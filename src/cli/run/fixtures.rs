//! Copy an eval's fixtures into the isolated env (`iteration-N/env/`), laid out
//! like a real repo so the agent-under-test reads them at natural project-relative
//! paths. One shared env hosts every eval's fixtures, so [`FixtureClaims`] dedups
//! idempotent re-declarations and rejects cross-eval clobbers.

use std::fs;
use std::path::Path;

use crate::core::Eval;

use super::{RunError, copy_entry};

/// Cross-eval claims on env-relative fixture destinations: `dest → (eval_id, source)`.
/// One shared `env/` hosts every eval's fixtures, so two evals targeting the same path
/// from *different* sources is an ambiguous, order-dependent clobber — [`claim_fixture_dest`]
/// rejects it. Same source is an idempotent re-declaration (the common shared-fixture case).
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

/// Reject a fixture path that is absolute or escapes `env/` via `..`, so a fixture
/// always lands inside the isolated env.
fn validate_fixture_rel(f: &str) -> Result<(), RunError> {
    let p = Path::new(f);
    let escapes = p.is_absolute()
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
    if escapes {
        return Err(RunError::msg(format!(
            "fixture path must be relative and stay within env: {f}"
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
    let Some(files) = ev.files.as_ref().filter(|f| !f.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut pairs = Vec::with_capacity(files.len());
    for f in files {
        validate_fixture_rel(f)?;
        let src = skill_dir.join("evals").join(f);
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
            copy_entry(Path::new(source), &dst)?;
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
            assertions: None,
            skill_should_trigger: None,
            runs: None,
            isolation: None,
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

        for bad in ["../escape.txt", "/etc/passwd", "a/../../b.txt"] {
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
