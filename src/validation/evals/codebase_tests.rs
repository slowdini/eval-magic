use super::validate_evals_config;
use crate::core::{CodebaseSource, EvalsConfig};
use crate::validation::schema::{SchemaName, validate_against_schema};
use serde_json::{Value, json};

fn base() -> Value {
    json!({
        "skill_name": "demo",
        "codebase": { "path": "." },
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
fn rejects_isolation_with_the_retirement_message() {
    let mut config = base();
    config["evals"][0]["isolation"] = json!("isolated");
    let err = validate_evals_config(&config, "evals.json")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(
            "eval 'e1': field 'isolation' is no longer supported or needed; every eval run already uses a private environment"
        ),
        "error was: {err}"
    );
}

#[test]
fn isolation_error_wins_when_the_config_also_lacks_a_codebase() {
    let mut config = base();
    config.as_object_mut().unwrap().remove("codebase");
    config["evals"][0]["isolation"] = json!("shared");

    let err = validate_evals_config(&config, "evals.json")
        .unwrap_err()
        .to_string();

    assert!(err.contains("field 'isolation'"), "error was: {err}");
    assert!(!err.contains("no effective codebase"), "error was: {err}");
}

#[test]
fn rejects_an_eval_without_an_effective_codebase() {
    let mut config = base();
    config.as_object_mut().unwrap().remove("codebase");

    let err = validate_evals_config(&config, "evals.json")
        .unwrap_err()
        .to_string();

    assert!(
        err.contains(
            "eval 'e1': no effective codebase; set top-level 'codebase' or this eval's 'codebase'"
        ),
        "error was: {err}"
    );
}

#[test]
fn schema_requires_a_default_or_per_eval_codebase() {
    let mut config = base();
    config.as_object_mut().unwrap().remove("codebase");

    let result: Result<EvalsConfig, _> =
        validate_against_schema(SchemaName::Evals, &config, "evals.json");

    assert!(result.is_err());
}

#[test]
fn accepts_per_eval_codebases_without_a_default() {
    let mut config = base();
    config.as_object_mut().unwrap().remove("codebase");
    config["evals"][0]["codebase"] = json!({ "path": "../project" });

    let parsed = validate_evals_config(&config, "evals.json").unwrap();

    assert!(parsed.codebase.is_none());
    assert!(parsed.evals[0].codebase.is_some());
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
            exclude_skill_sources: false,
            ignore_files: None,
        })
    );
}

#[test]
fn accepts_a_per_eval_path_codebase_overriding_the_default() {
    let mut config = base();
    config["codebase"] = json!({ "url": "https://example.com/project.git", "ref": "main" });
    config["evals"][0]["codebase"] = json!({ "path": "../projects/legacy-service" });

    let parsed = validate_evals_config(&config, "evals.json").unwrap();

    assert_eq!(
        parsed.evals[0].codebase,
        Some(CodebaseSource::Path {
            path: "../projects/legacy-service".to_string(),
            exclude_skill_sources: false,
            ignore_files: None,
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
            exclude_skill_sources: false,
            ignore_files: None,
        })
    );
}

#[test]
fn accepts_codebase_skill_source_exclusion() {
    let mut config = base();
    config["codebase"] = json!({
        "path": "/srv/projects/legacy-service",
        "exclude_skill_sources": true
    });

    let parsed = validate_evals_config(&config, "evals.json").unwrap();
    let declared = serde_json::to_value(parsed.codebase.unwrap()).unwrap();

    assert_eq!(declared["exclude_skill_sources"], true);
}

#[test]
fn accepts_declared_ignore_files() {
    let mut config = base();
    config["codebase"] = json!({
        "path": "/srv/projects/legacy-service",
        "ignore_files": ["config/.prettierignore", ".stylelintignore"]
    });

    let parsed = validate_evals_config(&config, "evals.json").unwrap();

    assert_eq!(
        parsed.codebase.unwrap().ignore_files(),
        Some(
            [
                "config/.prettierignore".to_string(),
                ".stylelintignore".to_string()
            ]
            .as_slice()
        )
    );
}

#[test]
fn an_empty_ignore_files_list_is_the_opt_out_not_the_default() {
    let mut config = base();
    config["codebase"] = json!({ "path": ".", "ignore_files": [] });

    let parsed = validate_evals_config(&config, "evals.json").unwrap();

    assert_eq!(parsed.codebase.unwrap().ignore_files(), Some([].as_slice()));
}

#[test]
fn an_absent_ignore_files_list_leaves_detection_in_charge() {
    let parsed = validate_evals_config(&base(), "evals.json").unwrap();

    assert_eq!(parsed.codebase.unwrap().ignore_files(), None);
}

/// The runner writes these paths inside a task environment, so anything that
/// leaves it — an absolute path, a `..` hop, a blank entry — is refused before
/// a run can touch the host.
#[test]
fn rejects_ignore_file_paths_that_leave_the_task_environment() {
    for path in [
        "/etc/.prettierignore",
        "../.prettierignore",
        "a/../../b",
        "  ",
    ] {
        let mut config = base();
        config["codebase"] = json!({ "path": ".", "ignore_files": [path] });

        let err = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("ignore_files"),
            "path {path:?} was accepted or reported oddly: {err}"
        );
    }
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

        let mut config = base();
        config["evals"][0]["codebase"] = codebase;
        let error = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(error.contains("e1"), "{field}: error was: {error}");
        assert!(error.contains(field), "{field}: error was: {error}");
    }
}

/// A source is either Git or a local path; hybrid declarations are invalid.
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

/// A Git source needs an explicit ref so a run can be reproduced from its resolved SHA.
#[test]
fn rejects_a_git_codebase_without_a_ref() {
    let mut config = base();
    config["codebase"] = json!({ "url": "https://example.com/p.git" });

    let error = validate_evals_config(&config, "evals.json")
        .unwrap_err()
        .to_string();

    assert!(error.contains("ref"), "error was: {error}");
}
