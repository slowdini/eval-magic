//! High-level `evals.json` validation: structural schema check plus the
//! hand-rolled constraints draft-07 can't express.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use regex::Regex;
use serde_json::Value;

use crate::core::{Assertion, DeliverWhen, EvalsConfig};
use crate::validation::error::ValidationError;
use crate::validation::schema::{SchemaName, validate_against_schema};

/// Validate a parsed `evals.json`. Runs the structural schema check, then the
/// supplemental duplicate-`id`, command environment, and held-out path guards,
/// returning the typed config on success.
pub fn validate_evals_config(config: &Value, source: &str) -> Result<EvalsConfig, ValidationError> {
    validate_codebase_declarations(config, source)?;
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

        for (turn_index, turn) in ev.turns.as_deref().unwrap_or(&[]).iter().enumerate() {
            if turn.prompt.trim().is_empty() {
                return Err(ValidationError::InvalidConfig {
                    path: source.to_string(),
                    message: format!(
                        "eval '{}', turns[{turn_index}]: prompt must contain non-whitespace text",
                        ev.id
                    ),
                });
            }
            if turn.deliver_when == DeliverWhen::Always && turn.agent_response_matches.is_some() {
                return Err(ValidationError::InvalidConfig {
                    path: source.to_string(),
                    message: format!(
                        "eval '{}', turns[{turn_index}]: agent_response_matches is only valid when deliver_when is 'agent_asks'",
                        ev.id
                    ),
                });
            }
            if let Some(pattern) = &turn.agent_response_matches
                && let Err(error) = Regex::new(pattern)
            {
                return Err(ValidationError::InvalidConfig {
                    path: source.to_string(),
                    message: format!(
                        "eval '{}', turns[{turn_index}]: invalid agent_response_matches regex {pattern:?}: {error}",
                        ev.id
                    ),
                });
            }
        }

        let visible = ev.files.as_deref().unwrap_or(&[]);
        for assertion in ev.assertions.as_deref().unwrap_or(&[]) {
            if let Assertion::TranscriptCheck(check) = assertion {
                if check.check == "assistant_message_matches" && ev.turns.is_none() {
                    return Err(ValidationError::InvalidConfig {
                        path: source.to_string(),
                        message: format!(
                            "eval '{}', transcript_check '{}': assistant_message_matches \
                             requires a non-empty turns array",
                            ev.id, check.id
                        ),
                    });
                }
                if let Some(pattern) = &check.pattern
                    && let Err(error) = Regex::new(pattern)
                {
                    return Err(ValidationError::InvalidConfig {
                        path: source.to_string(),
                        message: format!(
                            "eval '{}', transcript_check '{}': invalid pattern regex \
                             {pattern:?}: {error}",
                            ev.id, check.id
                        ),
                    });
                }
            }
            let Assertion::CommandCheck(check) = assertion else {
                continue;
            };
            for (name, value) in check.env.as_ref().into_iter().flatten() {
                validate_environment_name(source, &ev.id, &check.id, "env", name)?;
                validate_environment_value(source, &ev.id, &check.id, "env", name, value)?;
            }
            for (name, values) in check.matrix.as_ref().into_iter().flatten() {
                validate_environment_name(source, &ev.id, &check.id, "matrix", name)?;
                for value in values {
                    validate_environment_value(source, &ev.id, &check.id, "matrix", name, value)?;
                }
            }
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

/// Name what is wrong with a `codebase` block before the schema reports only
/// that it matched neither `oneOf` branch.
///
/// This runs *ahead* of the structural check rather than beside it. The schema
/// still owns the contract — an editor validating `evals.json` against it gets
/// the full rules — but `oneOf` cannot explain itself: a git source missing its
/// `ref` reports the whole block as unmatched and never mentions `ref`. Only the
/// mistakes worth a sentence are repeated here, plus the whitespace rule
/// `minLength: 1` cannot express.
fn validate_codebase_declarations(config: &Value, source: &str) -> Result<(), ValidationError> {
    if let Some(codebase) = config.get("codebase") {
        validate_codebase(source, "codebase", codebase)?;
    }
    let evals = config.get("evals").and_then(Value::as_array);
    for (index, eval) in evals.into_iter().flatten().enumerate() {
        let Some(codebase) = eval.get("codebase") else {
            continue;
        };
        let id = eval
            .get("id")
            .and_then(Value::as_str)
            .map_or_else(|| format!("evals[{index}]"), str::to_string);
        validate_codebase(source, &format!("eval '{id}', codebase"), codebase)?;
    }
    Ok(())
}

fn validate_codebase(source: &str, label: &str, value: &Value) -> Result<(), ValidationError> {
    // A non-object is a plain type error the schema words perfectly well.
    let Some(fields) = value.as_object() else {
        return Ok(());
    };
    let invalid = |message: String| ValidationError::InvalidConfig {
        path: source.to_string(),
        message,
    };

    if fields.contains_key("url") && fields.contains_key("path") {
        return Err(invalid(format!(
            "{label}: declares both 'url' and 'path'; a codebase is sourced from one or the other"
        )));
    }
    if fields.contains_key("url") && !fields.contains_key("ref") {
        return Err(invalid(format!(
            "{label}: 'url' requires an explicit 'ref' (branch, tag, or commit SHA). The runner \
             records the resolved SHA, so an eval tracking a moving branch could not be re-run \
             against what it measured."
        )));
    }
    for field in ["url", "ref", "path"] {
        if let Some(Value::String(text)) = fields.get(field)
            && text.trim().is_empty()
        {
            return Err(invalid(format!(
                "{label}: '{field}' must contain non-whitespace text"
            )));
        }
    }
    Ok(())
}

fn validate_environment_name(
    source: &str,
    eval_id: &str,
    check_id: &str,
    field: &str,
    name: &str,
) -> Result<(), ValidationError> {
    if name.is_empty() || name.contains('=') || name.contains('\0') {
        return Err(ValidationError::InvalidConfig {
            path: source.to_string(),
            message: format!(
                "eval '{eval_id}', command_check '{check_id}': {field} environment variable name must be non-empty and contain neither '=' nor NUL: {name:?}"
            ),
        });
    }
    Ok(())
}

fn validate_environment_value(
    source: &str,
    eval_id: &str,
    check_id: &str,
    field: &str,
    name: &str,
    value: &str,
) -> Result<(), ValidationError> {
    if value.contains('\0') {
        return Err(ValidationError::InvalidConfig {
            path: source.to_string(),
            message: format!(
                "eval '{eval_id}', command_check '{check_id}': {field} environment variable {name:?} value must not contain NUL"
            ),
        });
    }
    Ok(())
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
    use crate::core::CodebaseSource;
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
    fn rejects_an_empty_files_root() {
        let mut config = base();
        config["evals"][0]["files_root"] = json!("");

        let error = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();

        assert!(error.contains("files_root"), "error was: {error}");
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
    fn accepts_ordered_scripted_follow_up_turns() {
        let mut config = base();
        config["evals"][0]["turns"] = json!([
            {
                "prompt": "Which users are affected?",
                "deliver_when": "always"
            },
            {
                "prompt": "They are all in US timezones and this is a date-only field.",
                "deliver_when": "agent_asks",
                "agent_response_matches": "(?i)(time ?zone|date-only)"
            }
        ]);

        let parsed = validate_evals_config(&config, "evals.json").unwrap();
        let turns = parsed.evals[0].turns.as_ref().unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].prompt, "Which users are affected?");
        assert_eq!(turns[0].deliver_when, crate::core::DeliverWhen::Always);
        assert_eq!(turns[1].deliver_when, crate::core::DeliverWhen::AgentAsks);
        assert_eq!(
            turns[1].agent_response_matches.as_deref(),
            Some("(?i)(time ?zone|date-only)")
        );
    }

    #[test]
    fn rejects_assistant_message_check_without_scripted_turns() {
        let mut config = base();
        config["evals"][0]["assertions"] = json!([{
            "id": "asked",
            "type": "transcript_check",
            "check": "assistant_message_matches",
            "pattern": "timezone"
        }]);
        let err = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("assistant_message_matches"), "{err}");
        assert!(err.contains("turns"), "{err}");
    }

    #[test]
    fn rejects_invalid_scripted_turn_shapes() {
        for (turn, expected) in [
            (json!({"prompt": "", "deliver_when": "always"}), "prompt"),
            (
                json!({"prompt": "answer", "deliver_when": "sometimes"}),
                "deliver_when",
            ),
            (
                json!({
                    "prompt": "answer",
                    "deliver_when": "always",
                    "agent_response_matches": "topic"
                }),
                "agent_response_matches",
            ),
            (
                json!({
                    "prompt": "answer",
                    "deliver_when": "agent_asks",
                    "agent_response_matches": "("
                }),
                "agent_response_matches",
            ),
        ] {
            let mut config = base();
            config["evals"][0]["turns"] = json!([turn]);
            let error = validate_evals_config(&config, "evals.json")
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{expected}: {error}");
        }
    }

    #[test]
    fn rejects_an_empty_scripted_turns_array() {
        let mut config = base();
        config["evals"][0]["turns"] = json!([]);
        let error = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(error.contains("turns"), "{error}");
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

    #[test]
    fn accepts_command_check_environment_and_matrix() {
        let mut config = base();
        config["evals"][0]["assertions"] = json!([{
            "id": "timezone-matrix",
            "type": "command_check",
            "command": "bun test",
            "env": {
                "CI": "1",
                "EMPTY": "",
                "TZ": "UTC"
            },
            "matrix": {
                "LOCALE": ["en_US", "de_DE"],
                "MODE": ["", "strict"],
                "TZ": ["UTC", "America/Los_Angeles"]
            }
        }]);

        let parsed = validate_evals_config(&config, "evals.json").unwrap();
        let crate::core::Assertion::CommandCheck(check) =
            &parsed.evals[0].assertions.as_ref().unwrap()[0]
        else {
            panic!("expected command_check");
        };
        assert_eq!(check.env.as_ref().unwrap()["CI"], "1");
        assert_eq!(check.env.as_ref().unwrap()["EMPTY"], "");
        assert_eq!(check.matrix.as_ref().unwrap()["MODE"][0], "");
        assert_eq!(
            check.matrix.as_ref().unwrap()["TZ"],
            ["UTC", "America/Los_Angeles"]
        );
    }

    #[test]
    fn rejects_invalid_command_check_environment_names_and_nul_values() {
        for (field, value) in [
            ("env", json!({ "BAD=NAME": "value" })),
            ("env", json!({ "GOOD_NAME": "bad\u{0}value" })),
            ("matrix", json!({ "BAD=NAME": ["value"] })),
            ("matrix", json!({ "GOOD_NAME": ["bad\u{0}value"] })),
        ] {
            let mut config = base();
            config["evals"][0]["assertions"] = json!([{
                "id": "environment",
                "type": "command_check",
                "command": "true",
                field: value
            }]);

            let error = validate_evals_config(&config, "evals.json")
                .unwrap_err()
                .to_string();
            assert!(error.contains(field), "{field}: {error}");
            assert!(error.contains("environment"), "{field}: {error}");
        }
    }

    #[test]
    fn rejects_empty_or_duplicate_command_check_environment_collections() {
        for (field, value) in [
            ("env", json!({})),
            ("matrix", json!({})),
            ("matrix", json!({ "TZ": [] })),
            ("matrix", json!({ "TZ": ["UTC", "UTC"] })),
        ] {
            let mut config = base();
            config["evals"][0]["assertions"] = json!([{
                "id": "environment",
                "type": "command_check",
                "command": "true",
                field: value
            }]);

            let error = validate_evals_config(&config, "evals.json")
                .unwrap_err()
                .to_string();
            assert!(error.contains(field), "{field}: {error}");
        }
    }

    #[test]
    fn rejects_non_string_command_check_environment_values() {
        for (field, value) in [
            ("env", json!({ "CI": 1 })),
            ("env", json!({ "CI": null })),
            ("matrix", json!({ "TZ": ["UTC", 1] })),
            ("matrix", json!({ "TZ": "UTC" })),
        ] {
            let mut config = base();
            config["evals"][0]["assertions"] = json!([{
                "id": "environment",
                "type": "command_check",
                "command": "true",
                field: value
            }]);

            let error = validate_evals_config(&config, "evals.json")
                .unwrap_err()
                .to_string();
            assert!(error.contains(field), "{field}: {error}");
        }
    }

    #[test]
    fn accepts_diff_scope_with_either_or_both_thresholds() {
        for assertion in [
            json!({
                "id": "files",
                "type": "diff_scope",
                "max_files_touched": 1
            }),
            json!({
                "id": "lines",
                "type": "diff_scope",
                "max_lines_changed": 8
            }),
            json!({
                "id": "both",
                "type": "diff_scope",
                "max_files_touched": 1,
                "max_lines_changed": 8
            }),
        ] {
            let mut config = base();
            config["evals"][0]["assertions"] = json!([assertion]);
            validate_evals_config(&config, "evals.json").unwrap();
        }
    }

    #[test]
    fn rejects_diff_scope_without_a_threshold() {
        let mut config = base();
        config["evals"][0]["assertions"] = json!([{
            "id": "report-only",
            "type": "diff_scope"
        }]);
        let error = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(error.contains("diff_scope"), "{error}");
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

    #[test]
    fn accepts_a_top_level_git_codebase_as_the_default() {
        let mut config = base();
        config["codebase"] = json!({ "url": "https://example.com/project.git", "ref": "main" });

        let parsed = validate_evals_config(&config, "evals.json").unwrap();

        assert_eq!(
            parsed.codebase,
            Some(CodebaseSource::Git {
                url: "https://example.com/project.git".to_string(),
                reference: "main".to_string(),
            })
        );
    }

    #[test]
    fn accepts_a_per_eval_path_codebase_overriding_the_default() {
        let mut config = base();
        config["codebase"] = json!({ "url": "https://example.com/project.git", "ref": "main" });
        config["evals"][0]["codebase"] = json!({ "path": "../fixtures/legacy-service" });

        let parsed = validate_evals_config(&config, "evals.json").unwrap();

        assert_eq!(
            parsed.evals[0].codebase,
            Some(CodebaseSource::Path {
                path: "../fixtures/legacy-service".to_string(),
            })
        );
    }

    #[test]
    fn accepts_a_top_level_path_codebase() {
        let mut config = base();
        config["codebase"] = json!({ "path": "/srv/projects/legacy-service" });

        let parsed = validate_evals_config(&config, "evals.json").unwrap();

        assert_eq!(
            parsed.codebase,
            Some(CodebaseSource::Path {
                path: "/srv/projects/legacy-service".to_string(),
            })
        );
    }

    /// `minLength: 1` admits `" "`, so the schema cannot carry this on its own.
    #[test]
    fn rejects_whitespace_only_codebase_values() {
        for (field, codebase) in [
            ("url", json!({ "url": "   ", "ref": "main" })),
            (
                "ref",
                json!({ "url": "https://example.com/p.git", "ref": "\t" }),
            ),
            ("path", json!({ "path": " " })),
        ] {
            let mut config = base();
            config["codebase"] = codebase.clone();
            let error = validate_evals_config(&config, "evals.json")
                .unwrap_err()
                .to_string();
            assert!(error.contains("codebase"), "{field}: error was: {error}");
            assert!(error.contains(field), "{field}: error was: {error}");

            // The per-eval override runs through the same guard, and names the eval.
            let mut config = base();
            config["evals"][0]["codebase"] = codebase;
            let error = validate_evals_config(&config, "evals.json")
                .unwrap_err()
                .to_string();
            assert!(error.contains("e1"), "{field}: error was: {error}");
            assert!(error.contains(field), "{field}: error was: {error}");
        }
    }

    /// A source is one thing or the other. The schema's `oneOf` plus
    /// `additionalProperties: false` on each branch is what rejects the hybrid;
    /// this pins that so a later schema edit cannot quietly admit it.
    #[test]
    fn rejects_a_codebase_that_is_both_git_and_path() {
        let mut config = base();
        config["codebase"] = json!({
            "url": "https://example.com/p.git",
            "ref": "main",
            "path": "/srv/p"
        });

        assert!(validate_evals_config(&config, "evals.json").is_err());
    }

    /// #244 decision 5: the runner records the resolved SHA, so a git source
    /// without an explicit ref could not be re-run against what it measured.
    #[test]
    fn rejects_a_git_codebase_without_a_ref() {
        let mut config = base();
        config["codebase"] = json!({ "url": "https://example.com/p.git" });

        let error = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();

        assert!(error.contains("ref"), "error was: {error}");
    }
}
