//! High-level `evals.json` validation: structural schema check plus the
//! hand-rolled constraints draft-07 can't express.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::core::{Assertion, EvalsConfig};
use crate::validation::error::ValidationError;
use crate::validation::schema::{SchemaName, validate_against_schema};

/// Validate a parsed `evals.json`. Runs the structural schema check, then the
/// supplemental duplicate-`id` guard (uniqueness by a sub-field isn't
/// expressible in JSON Schema draft-07), returning the typed config on success.
pub fn validate_evals_config(config: &Value, source: &str) -> Result<EvalsConfig, ValidationError> {
    let validated: EvalsConfig = validate_against_schema(SchemaName::Evals, config, source)?;

    let mut seen = HashSet::new();
    for (index, ev) in validated.evals.iter().enumerate() {
        if !seen.insert(ev.id.as_str()) {
            return Err(ValidationError::DuplicateId {
                path: source.to_string(),
                index,
                id: ev.id.clone(),
            });
        }

        let visible = ev.files.as_deref().unwrap_or(&[]);
        for assertion in ev.assertions.as_deref().unwrap_or(&[]) {
            let Assertion::CommandCheck(check) = assertion else {
                continue;
            };
            for setup in check.setup_files.as_deref().unwrap_or(&[]) {
                let setup_path = normalize_relative(setup).map_err(|()| {
                    ValidationError::InvalidConfig {
                        path: source.to_string(),
                        message: format!(
                            "eval '{}', command_check '{}': setup_files path must be relative and stay within the task environment: {setup}",
                            ev.id, check.id
                        ),
                    }
                })?;
                for fixture in visible {
                    let Ok(fixture_path) = normalize_relative(fixture) else {
                        continue;
                    };
                    if paths_overlap(&fixture_path, &setup_path) {
                        return Err(ValidationError::InvalidConfig {
                            path: source.to_string(),
                            message: format!(
                                "eval '{}', command_check '{}': visible fixture '{}' and setup_files path '{}' overlap; held-out setup paths must be disjoint from agent-visible files",
                                ev.id, check.id, fixture, setup
                            ),
                        });
                    }
                }
            }
        }
    }

    Ok(validated)
}

fn normalize_relative(value: &str) -> Result<PathBuf, ()> {
    let mut normalized = PathBuf::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return Err(()),
        }
    }
    Ok(normalized)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
mod tests {
    use super::validate_evals_config;
    use serde_json::{Value, json};

    /// The minimal valid config the cases below mutate.
    fn base() -> Value {
        json!({
            "skill_name": "demo",
            "evals": [
                {
                    "id": "e1",
                    "prompt": "do the thing",
                    "expected_output": "the thing is done"
                }
            ]
        })
    }

    #[test]
    fn accepts_a_boolean_skill_should_trigger() {
        let mut config = base();
        config["evals"][0]["skill_should_trigger"] = json!(false);
        let parsed = validate_evals_config(&config, "evals.json").unwrap();
        assert_eq!(parsed.evals[0].skill_should_trigger, Some(false));
    }

    #[test]
    fn accepts_evals_with_no_skill_should_trigger() {
        let config = base();
        let parsed = validate_evals_config(&config, "evals.json").unwrap();
        assert_eq!(parsed.skill_name, "demo");
        assert_eq!(parsed.evals[0].skill_should_trigger, None);
    }

    #[test]
    fn rejects_a_non_boolean_skill_should_trigger() {
        let mut config = base();
        config["evals"][0]["skill_should_trigger"] = json!("false");
        let err = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("skill_should_trigger"), "error was: {err}");
    }

    #[test]
    fn accepts_isolation_isolated() {
        let mut config = base();
        config["evals"][0]["isolation"] = json!("isolated");
        let parsed = validate_evals_config(&config, "evals.json").unwrap();
        assert_eq!(
            parsed.evals[0].isolation,
            Some(crate::core::Isolation::Isolated)
        );
    }

    #[test]
    fn accepts_isolation_shared() {
        let mut config = base();
        config["evals"][0]["isolation"] = json!("shared");
        let parsed = validate_evals_config(&config, "evals.json").unwrap();
        assert_eq!(
            parsed.evals[0].isolation,
            Some(crate::core::Isolation::Shared)
        );
    }

    #[test]
    fn defaults_isolation_to_none_when_absent() {
        let config = base();
        let parsed = validate_evals_config(&config, "evals.json").unwrap();
        assert_eq!(parsed.evals[0].isolation, None);
    }

    #[test]
    fn rejects_an_unknown_isolation_value() {
        let mut config = base();
        config["evals"][0]["isolation"] = json!("sometimes");
        let err = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("isolation"), "error was: {err}");
    }

    #[test]
    fn rejects_a_non_kebab_case_id() {
        let mut config = base();
        config["evals"][0]["id"] = json!("Not Kebab");
        assert!(validate_evals_config(&config, "evals.json").is_err());
    }

    #[test]
    fn rejects_duplicate_eval_ids() {
        let mut config = base();
        let dup = config["evals"][0].clone();
        config["evals"] = json!([dup.clone(), dup]);
        let err = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate"), "error was: {err}");
    }

    #[test]
    fn rejects_an_empty_evals_array() {
        let mut config = base();
        config["evals"] = json!([]);
        assert!(validate_evals_config(&config, "evals.json").is_err());
    }

    #[test]
    fn accepts_command_check_with_optional_setup_stdout_and_exit_code() {
        let mut config = base();
        config["evals"][0]["assertions"] = json!([
            {
                "id": "default-exit",
                "type": "command_check",
                "command": "cargo test"
            },
            {
                "id": "full",
                "type": "command_check",
                "setup_files": ["holdout/test.rs"],
                "command": "cargo test --test holdout",
                "expect_exit_code": 2,
                "expect_stdout": "2 tests passed"
            }
        ]);

        let parsed = validate_evals_config(&config, "evals.json").unwrap();
        let assertions = parsed.evals[0].assertions.as_ref().unwrap();
        let crate::core::Assertion::CommandCheck(defaulted) = &assertions[0] else {
            panic!("expected command_check");
        };
        assert_eq!(defaulted.expect_exit_code, 0);
        let crate::core::Assertion::CommandCheck(full) = &assertions[1] else {
            panic!("expected command_check");
        };
        assert_eq!(
            full.setup_files.as_deref(),
            Some(&["holdout/test.rs".into()][..])
        );
        assert_eq!(full.expect_exit_code, 2);
        assert_eq!(full.expect_stdout.as_deref(), Some("2 tests passed"));
    }

    fn with_command_check(files: &[&str], setup_files: &[&str]) -> Value {
        let mut config = base();
        config["evals"][0]["files"] = json!(files);
        config["evals"][0]["assertions"] = json!([{
            "id": "held-out",
            "type": "command_check",
            "setup_files": setup_files,
            "command": "test -f holdout/test.txt"
        }]);
        config
    }

    #[test]
    fn rejects_exact_visible_and_setup_file_overlap() {
        let config = with_command_check(&["holdout/test.txt"], &["holdout/test.txt"]);
        let err = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("overlap"), "error was: {err}");
        assert!(err.contains("holdout/test.txt"), "error was: {err}");
    }

    #[test]
    fn rejects_visible_directory_ancestor_of_setup_file() {
        let config = with_command_check(&["holdout"], &["holdout/test.txt"]);
        let err = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("overlap"), "error was: {err}");
        assert!(err.contains("holdout"), "error was: {err}");
    }

    #[test]
    fn rejects_setup_directory_ancestor_of_visible_file() {
        let config = with_command_check(&["src/main.rs"], &["src"]);
        let err = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("overlap"), "error was: {err}");
        assert!(err.contains("src"), "error was: {err}");
    }

    #[test]
    fn rejects_absolute_and_escaping_setup_paths() {
        for setup in ["/tmp/holdout.txt", "../holdout.txt", "holdout/../../escape"] {
            let config = with_command_check(&["src/main.rs"], &[setup]);
            let err = validate_evals_config(&config, "evals.json")
                .unwrap_err()
                .to_string();
            assert!(err.contains("setup_files"), "{setup}: {err}");
            assert!(err.contains("relative"), "{setup}: {err}");
        }
    }

    #[test]
    fn accepts_disjoint_visible_and_setup_paths() {
        let config = with_command_check(&["src/main.rs"], &["holdout/test.txt"]);
        validate_evals_config(&config, "evals.json").unwrap();
    }
}
