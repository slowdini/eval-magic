use super::{SchemaName, Value, json, validate_against_schema};

fn benchmark_with_assertion_count(passed: i64, n: i64) -> Value {
    json!({
        "generated": "2026-06-08T00:00:00.000Z",
        "mode": "new-skill",
        "conditions_compared": ["with_skill", "without_skill"],
        "missing_gradings": 0,
        "validity_warnings": [],
        "run_summary": {},
        "assertions": {
            "case-a": {
                "behavior": {
                    "with_skill": { "passed": passed, "n": n }
                }
            }
        },
        "delta": {
            "direction": "with_skill - without_skill",
            "pass_rate": 0.0,
            "duration_ms": 0.0,
            "total_tokens": 0.0
        }
    })
}

#[test]
fn assertion_counts_require_an_observed_sample() {
    let benchmark = benchmark_with_assertion_count(0, 0);
    let result: Result<Value, _> =
        validate_against_schema(SchemaName::Benchmark, &benchmark, "benchmark.json");

    assert!(result.is_err(), "zero-sample cells must be omitted");
}

#[test]
fn assertion_counts_reject_negative_passes() {
    let benchmark = benchmark_with_assertion_count(-1, 1);
    let result: Result<Value, _> =
        validate_against_schema(SchemaName::Benchmark, &benchmark, "benchmark.json");

    assert!(result.is_err(), "passed counts cannot be negative");
}
