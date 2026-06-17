//! Batch validation of every `<skill>/evals/evals.json` under a skills dir.
//!
//! Pure logic: it returns a [`ValidationReport`] instead of printing. The CLI
//! handler renders the report (the `✓`/`✗` lines, the summary, the exit code),
//! which keeps this batch logic unit-testable without capturing stdout.

use std::fs;
use std::io;
use std::path::Path;

use crate::validation::evals::validate_evals_config;

/// The result of validating one `<skill>/evals/evals.json`.
#[derive(Debug, Clone)]
pub struct FileOutcome {
    /// The skill directory name (used to label summary lines).
    pub skill: String,
    /// Display path relative to the skills dir: `<skill>/evals/evals.json`.
    pub rel_path: String,
    /// `None` if the file validated; otherwise the failure message.
    pub error: Option<String>,
}

/// The outcome of a batch run, one entry per skill that had an `evals.json`.
#[derive(Debug, Default)]
pub struct ValidationReport {
    pub outcomes: Vec<FileOutcome>,
}

impl ValidationReport {
    /// How many files validated cleanly.
    pub fn validated(&self) -> usize {
        self.outcomes.iter().filter(|o| o.error.is_none()).count()
    }

    /// How many files failed validation.
    pub fn failed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.error.is_some()).count()
    }
}

/// Validate `<skill>/evals/evals.json` for every immediate child directory of
/// `skill_dir` that has one. Directories without an `evals.json` are skipped;
/// a parse or validation failure is recorded as a [`FileOutcome`] rather than
/// aborting the batch. Skills are processed in sorted order for deterministic
/// output.
pub fn validate_all(skill_dir: &Path) -> io::Result<ValidationReport> {
    // `entry.path().is_dir()` follows symlinks (matching the original's
    // `statSync().isDirectory()`), so a symlinked skill directory is still seen.
    let mut skills: Vec<String> = fs::read_dir(skill_dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry
                .path()
                .is_dir()
                .then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect();
    skills.sort();

    let mut report = ValidationReport::default();
    for skill in skills {
        let evals_path = skill_dir.join(&skill).join("evals").join("evals.json");
        if !evals_path.exists() {
            continue;
        }

        let rel_path = format!("{skill}/evals/evals.json");
        let error = match fs::read_to_string(&evals_path) {
            Err(e) => Some(format!("{rel_path}: {e}")),
            Ok(contents) => match serde_json::from_str(&contents) {
                Err(e) => Some(format!("{rel_path}: invalid JSON: {e}")),
                Ok(raw) => validate_evals_config(&raw, &rel_path)
                    .err()
                    .map(|e| e.to_string()),
            },
        };

        report.outcomes.push(FileOutcome {
            skill,
            rel_path,
            error,
        });
    }

    Ok(report)
}

/// Validate exactly one skill's `evals/evals.json`, reporting paths relative to
/// that skill directory.
pub fn validate_one(skill_subdir: &Path) -> io::Result<ValidationReport> {
    let skill = skill_subdir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "skill".to_string());
    let evals_path = skill_subdir.join("evals").join("evals.json");
    let rel_path = "evals/evals.json".to_string();
    let error = match fs::read_to_string(&evals_path) {
        Err(e) => Some(format!("{rel_path}: {e}")),
        Ok(contents) => match serde_json::from_str(&contents) {
            Err(e) => Some(format!("{rel_path}: invalid JSON: {e}")),
            Ok(raw) => validate_evals_config(&raw, &rel_path)
                .err()
                .map(|e| e.to_string()),
        },
    };

    Ok(ValidationReport {
        outcomes: vec![FileOutcome {
            skill,
            rel_path,
            error,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::validate_all;
    use std::fs;
    use tempfile::TempDir;

    /// A minimal valid `evals.json` body.
    const VALID: &str = r#"{ "skill_name": "demo", "evals": [
        { "id": "e1", "prompt": "p", "expected_output": "o" } ] }"#;

    /// Write `<root>/<skill>/evals/evals.json` with the given contents.
    fn write_evals(root: &std::path::Path, skill: &str, contents: &str) {
        let dir = root.join(skill).join("evals");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("evals.json"), contents).unwrap();
    }

    #[test]
    fn reports_one_outcome_per_skill_with_an_evals_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_evals(root, "good", VALID);
        write_evals(root, "bad", r#"{ "skill_name": "x", "evals": [] }"#); // empty evals
        write_evals(root, "broken", "{ not json"); // parse failure
        // A skill directory with no evals.json is skipped entirely.
        fs::create_dir_all(root.join("nofile")).unwrap();
        // A plain file at the top level is not a skill directory.
        fs::write(root.join("README.md"), "hi").unwrap();

        let report = validate_all(root).unwrap();

        assert_eq!(report.validated(), 1);
        assert_eq!(report.failed(), 2);

        let good = report.outcomes.iter().find(|o| o.skill == "good").unwrap();
        assert!(good.error.is_none());
        assert_eq!(good.rel_path, "good/evals/evals.json");

        let bad = report.outcomes.iter().find(|o| o.skill == "bad").unwrap();
        assert!(bad.error.is_some());

        let broken = report
            .outcomes
            .iter()
            .find(|o| o.skill == "broken")
            .unwrap();
        assert!(broken.error.is_some());

        assert!(report.outcomes.iter().all(|o| o.skill != "nofile"));
        assert!(report.outcomes.iter().all(|o| o.skill != "README.md"));
    }

    #[test]
    fn an_all_valid_dir_has_no_failures() {
        let tmp = TempDir::new().unwrap();
        write_evals(tmp.path(), "a", VALID);
        write_evals(tmp.path(), "b", VALID);

        let report = validate_all(tmp.path()).unwrap();

        assert_eq!(report.validated(), 2);
        assert_eq!(report.failed(), 0);
        assert!(report.outcomes.iter().all(|o| o.error.is_none()));
    }
}
