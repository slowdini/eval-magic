//! Schema embedding + the generic `validate_against_schema` entry point.
//!
//! The portable-artifact schemas are the single source of truth for each
//! artifact's shape. They are embedded at compile time with `include_str!` (so
//! the binary is self-contained, with no `schema/` directory to ship alongside)
//! and compiled once into reusable `jsonschema` validators.

use std::collections::HashMap;
use std::sync::LazyLock;

use jsonschema::Validator;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::validation::error::ValidationError;

/// Names the portable-artifact schemas. `benchmark` and `judge-tasks` are
/// first-class here, schema-gated like every other pipeline output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaName {
    RunRecord,
    Evals,
    Grading,
    StrayWrites,
    Benchmark,
    JudgeTasks,
}

impl SchemaName {
    /// Every schema, for building the validator cache.
    const ALL: [SchemaName; 6] = [
        SchemaName::RunRecord,
        SchemaName::Evals,
        SchemaName::Grading,
        SchemaName::StrayWrites,
        SchemaName::Benchmark,
        SchemaName::JudgeTasks,
    ];

    /// The schema's kebab-case name, as used in error messages and the on-disk
    /// `<name>.schema.json` filenames.
    pub fn as_str(self) -> &'static str {
        match self {
            SchemaName::RunRecord => "run-record",
            SchemaName::Evals => "evals",
            SchemaName::Grading => "grading",
            SchemaName::StrayWrites => "stray-writes",
            SchemaName::Benchmark => "benchmark",
            SchemaName::JudgeTasks => "judge-tasks",
        }
    }

    /// The embedded schema JSON source.
    fn source(self) -> &'static str {
        match self {
            SchemaName::RunRecord => include_str!("../../schema/run-record.schema.json"),
            SchemaName::Evals => include_str!("../../schema/evals.schema.json"),
            SchemaName::Grading => include_str!("../../schema/grading.schema.json"),
            SchemaName::StrayWrites => include_str!("../../schema/stray-writes.schema.json"),
            SchemaName::Benchmark => include_str!("../../schema/benchmark.schema.json"),
            SchemaName::JudgeTasks => include_str!("../../schema/judge-tasks.schema.json"),
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
/// Deserializing into `T` makes the typed result honest: the schema gate and
/// the Rust type agree, or the call fails.
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

    /// The canonical valid run-record the cases below mutate.
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
    fn validates_a_well_formed_benchmark() {
        let benchmark = json!({
            "generated": "2026-06-08T00:00:00.000Z",
            "mode": "new-skill",
            "conditions_compared": ["with_skill", "without_skill"],
            "missing_gradings": 0,
            "validity_warnings": [],
            "run_summary": {
                "with_skill": {
                    "pass_rate": { "mean": 1.0, "stddev": 0.0, "n": 1 },
                    "duration_ms": { "mean": 1000.0, "stddev": 0.0, "n": 1 },
                    "total_tokens": { "mean": 5000.0, "stddev": 0.0, "n": 1 },
                    "skill_invocation_n": 0,
                    "skill_invocation_rate": null
                },
                "without_skill": {
                    "pass_rate": { "mean": 0.0, "stddev": 0.0, "n": 1 },
                    "duration_ms": { "mean": 1000.0, "stddev": 0.0, "n": 1 },
                    "total_tokens": { "mean": 3000.0, "stddev": 0.0, "n": 1 }
                }
            },
            "delta": { "direction": "with_skill - without_skill", "pass_rate": 1.0, "duration_ms": 0.0, "total_tokens": 2000.0 }
        });
        let r: Result<Value, _> =
            validate_against_schema(SchemaName::Benchmark, &benchmark, "benchmark.json");
        assert!(r.is_ok(), "benchmark should validate: {r:?}");
    }

    #[test]
    fn rejects_a_benchmark_missing_delta() {
        let mut benchmark = json!({
            "generated": "t", "mode": "new-skill",
            "conditions_compared": ["a", "b"], "missing_gradings": 0,
            "validity_warnings": [], "run_summary": {},
            "delta": { "direction": "a - b", "pass_rate": 0.0, "duration_ms": 0.0, "total_tokens": 0.0 }
        });
        benchmark.as_object_mut().unwrap().remove("delta");
        let r: Result<Value, _> =
            validate_against_schema(SchemaName::Benchmark, &benchmark, "benchmark.json");
        assert!(r.is_err());
    }

    #[test]
    fn validates_a_well_formed_judge_tasks_file() {
        let tasks = json!({
            "generated": "2026-06-08T00:00:00.000Z",
            "total_tasks": 1,
            "meta_tasks_injected": 1,
            "skipped_transcript_checks": 0,
            "tasks": [{
                "eval_id": "e1", "condition": "with_skill", "assertion_id": "__skill_invoked",
                "rubric": "did it apply the skill?", "model": null, "is_meta": true,
                "run_record_path": "/w/run.json", "outputs_dir": "/w/outputs",
                "response_path": "/w/judge-responses/__skill_invoked.json",
                "dispatch_prompt_path": "/w/judge-prompts/__skill_invoked.txt"
            }]
        });
        let r: Result<Value, _> =
            validate_against_schema(SchemaName::JudgeTasks, &tasks, "judge-tasks.json");
        assert!(r.is_ok(), "judge-tasks should validate: {r:?}");
    }

    #[test]
    fn rejects_a_judge_task_with_an_inlined_dispatch_prompt() {
        // dispatch_prompt is stripped before write; the schema forbids extras.
        let tasks = json!({
            "generated": "t", "total_tasks": 1, "meta_tasks_injected": 0,
            "skipped_transcript_checks": 0,
            "tasks": [{
                "eval_id": "e1", "condition": "with_skill", "assertion_id": "a1",
                "rubric": "r", "model": null, "is_meta": false,
                "run_record_path": "/w/run.json", "outputs_dir": "/w/outputs",
                "response_path": "/w/r.json", "dispatch_prompt_path": "/w/p.txt",
                "dispatch_prompt": "SHOULD NOT BE HERE"
            }]
        });
        let r: Result<Value, _> =
            validate_against_schema(SchemaName::JudgeTasks, &tasks, "judge-tasks.json");
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
