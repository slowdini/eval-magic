//! Schema embedding + the generic `validate_against_schema` entry point.
//!
//! The four portable-artifact schemas are the single source of truth for each
//! artifact's shape. They are embedded at compile time with `include_str!` (so
//! the binary is self-contained, with no `schema/` directory to ship alongside)
//! and compiled once into reusable `jsonschema` validators — the Rust analogue
//! of eval-runner's lazily-populated AJV validator `Map`.

use std::collections::HashMap;
use std::sync::LazyLock;

use jsonschema::Validator;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::validation::error::ValidationError;

/// Names the four portable-artifact schemas. Mirrors the `SchemaName` string
/// union in eval-runner's `validate-schema.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaName {
    RunRecord,
    Evals,
    Grading,
    StrayWrites,
}

impl SchemaName {
    /// Every schema, for building the validator cache.
    const ALL: [SchemaName; 4] = [
        SchemaName::RunRecord,
        SchemaName::Evals,
        SchemaName::Grading,
        SchemaName::StrayWrites,
    ];

    /// The schema's kebab-case name, as used in error messages and the on-disk
    /// `<name>.schema.json` filenames.
    pub fn as_str(self) -> &'static str {
        match self {
            SchemaName::RunRecord => "run-record",
            SchemaName::Evals => "evals",
            SchemaName::Grading => "grading",
            SchemaName::StrayWrites => "stray-writes",
        }
    }

    /// The embedded schema JSON source.
    fn source(self) -> &'static str {
        match self {
            SchemaName::RunRecord => include_str!("../../schema/run-record.schema.json"),
            SchemaName::Evals => include_str!("../../schema/evals.schema.json"),
            SchemaName::Grading => include_str!("../../schema/grading.schema.json"),
            SchemaName::StrayWrites => include_str!("../../schema/stray-writes.schema.json"),
        }
    }
}

/// Compiled validators, built once on first use. The schemas are embedded and
/// known-valid, so a failure here is a programmer error (a malformed bundled
/// schema) and panics rather than being surfaced as a runtime validation error.
static VALIDATORS: LazyLock<HashMap<SchemaName, Validator>> = LazyLock::new(|| {
    SchemaName::ALL
        .iter()
        .map(|&name| {
            let schema: Value = serde_json::from_str(name.source()).unwrap_or_else(|e| {
                panic!("bundled {} schema is not valid JSON: {e}", name.as_str())
            });
            let validator = jsonschema::validator_for(&schema).unwrap_or_else(|e| {
                panic!("bundled {} schema does not compile: {e}", name.as_str())
            });
            (name, validator)
        })
        .collect()
});

/// Validate `data` against the named schema. Returns it deserialized into `T` on
/// success; on mismatch, returns a `source`-prefixed [`ValidationError`] listing
/// every failure.
///
/// Deserializing into `T` (rather than eval-runner's bare `data as T` cast) makes
/// the typed result honest: the schema gate and the Rust type agree, or the call
/// fails.
pub fn validate_against_schema<T: DeserializeOwned>(
    name: SchemaName,
    data: &Value,
    source: &str,
) -> Result<T, ValidationError> {
    let validator = &VALIDATORS[&name];

    if !validator.is_valid(data) {
        let details = validator
            .iter_errors(data)
            .map(|e| {
                let instance = e.instance_path().to_string();
                let instance = if instance.is_empty() {
                    "/".to_string()
                } else {
                    instance
                };
                format!("  {instance} {e}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ValidationError::SchemaMismatch {
            path: source.to_string(),
            schema: name.as_str().to_string(),
            details,
        });
    }

    serde_json::from_value(data.clone()).map_err(|e| ValidationError::Deserialize {
        path: source.to_string(),
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{SchemaName, validate_against_schema};
    use serde_json::{Value, json};

    /// The canonical valid run-record used across these cases — mirrors the
    /// `validRunRecord` fixture in eval-runner's `validate-schema.test.ts`.
    fn valid_run_record() -> Value {
        json!({
            "eval_id": "e1",
            "condition": "with_skill",
            "skill_path": null,
            "prompt": "do the thing",
            "files": [],
            "final_message": "done",
            "tool_invocations": [],
            "total_tokens": 100,
            "duration_ms": 1000
        })
    }

    #[test]
    fn returns_data_when_it_matches_the_run_record_schema() {
        let data = valid_run_record();
        let out: Value =
            validate_against_schema(SchemaName::RunRecord, &data, "/tmp/run.json").unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn accepts_an_empty_tool_invocations_array() {
        let mut data = valid_run_record();
        data["tool_invocations"] = json!([]);
        let r: Result<Value, _> = validate_against_schema(SchemaName::RunRecord, &data, "run.json");
        assert!(r.is_ok());
    }

    #[test]
    fn accepts_skill_path_null_on_the_without_skill_arm() {
        let mut data = valid_run_record();
        data["condition"] = json!("without_skill");
        data["skill_path"] = Value::Null;
        let r: Result<Value, _> = validate_against_schema(SchemaName::RunRecord, &data, "run.json");
        assert!(r.is_ok());
    }

    #[test]
    fn source_prefixed_error_when_required_field_missing() {
        let mut data = valid_run_record();
        data.as_object_mut().unwrap().remove("eval_id");
        let err = validate_against_schema::<Value>(SchemaName::RunRecord, &data, "/tmp/run.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("/tmp/run.json"), "error was: {err}");
    }

    #[test]
    fn requires_skill_path() {
        let mut data = valid_run_record();
        data.as_object_mut().unwrap().remove("skill_path");
        let err = validate_against_schema::<Value>(SchemaName::RunRecord, &data, "run.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("skill_path"), "error was: {err}");
    }

    #[test]
    fn requires_files() {
        let mut data = valid_run_record();
        data.as_object_mut().unwrap().remove("files");
        let err = validate_against_schema::<Value>(SchemaName::RunRecord, &data, "run.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("files"), "error was: {err}");
    }

    #[test]
    fn rejects_unknown_extra_property() {
        let mut data = valid_run_record();
        data["surprise"] = json!(true);
        let r: Result<Value, _> = validate_against_schema(SchemaName::RunRecord, &data, "run.json");
        assert!(r.is_err());
    }

    #[test]
    fn tool_invocation_ordinal_must_be_an_integer() {
        let mut data = valid_run_record();
        data["tool_invocations"] = json!([{ "name": "Bash", "ordinal": "zero" }]);
        let r: Result<Value, _> = validate_against_schema(SchemaName::RunRecord, &data, "run.json");
        assert!(r.is_err());
    }

    #[test]
    fn compiles_and_validates_the_grading_schema_too() {
        let grading = json!({
            "assertion_results": [
                {
                    "id": "a1",
                    "passed": true,
                    "evidence": "quoted output",
                    "grader": "transcript_check"
                }
            ],
            "summary": { "passed": 1, "failed": 0, "total": 1, "pass_rate": 1.0 }
        });
        let r: Result<Value, _> =
            validate_against_schema(SchemaName::Grading, &grading, "grading.json");
        assert!(r.is_ok(), "grading should validate");
    }
}
