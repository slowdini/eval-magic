//! Timing rollups, missing samples, and per-metric provenance warnings.

use super::*;
use serde_json::json;

#[test]
fn aggregate_warns_on_mixed_metric_sources() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    write_grading(&iteration_dir, "with_skill", 1.0);
    write_timing(
        &iteration_dir,
        "with_skill",
        json!({"total_tokens": 5000, "duration_ms": 1000}),
    );
    write_grading(&iteration_dir, "without_skill", 1.0);
    write_timing(
        &iteration_dir,
        "without_skill",
        json!({
            "total_tokens": 90000,
            "duration_ms": 1200,
            "token_source": "transcript",
            "duration_source": "runner"
        }),
    );

    agg_cmd(&cwd, &skill_dir).assert().success();

    let benchmark = read_benchmark(&iteration_dir);
    let warnings = benchmark["validity_warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|warning| {
        let warning = warning.as_str().unwrap();
        warning.contains("total_tokens sources")
            && warning.contains("completion-event")
            && warning.contains("transcript")
    }));
    assert!(warnings.iter().any(|warning| {
        let warning = warning.as_str().unwrap();
        warning.contains("duration_ms sources")
            && warning.contains("completion-event")
            && warning.contains("runner")
    }));
}

#[test]
fn aggregate_no_warning_when_each_metric_source_matches() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    for condition in ["with_skill", "without_skill"] {
        write_grading(&iteration_dir, condition, 1.0);
        write_timing(
            &iteration_dir,
            condition,
            json!({
                "total_tokens": 100,
                "duration_ms": 1,
                "token_source": "transcript",
                "duration_source": "runner"
            }),
        );
    }

    agg_cmd(&cwd, &skill_dir).assert().success();

    let benchmark = read_benchmark(&iteration_dir);
    let warnings = benchmark["validity_warnings"].as_array().unwrap();
    assert!(
        !warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("sources ("))
    );
}

/// Incomplete samples are explicit, especially `n: 0`, whose numeric zero
/// mean is retained only for schema compatibility.
#[test]
fn aggregate_warns_when_token_or_duration_samples_are_missing() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    write_grading(&iteration_dir, "with_skill", 1.0);
    write_timing(
        &iteration_dir,
        "with_skill",
        json!({"total_tokens": 100, "duration_ms": null, "source": "transcript"}),
    );
    write_grading(&iteration_dir, "without_skill", 1.0);

    agg_cmd(&cwd, &skill_dir).assert().success();

    let benchmark = read_benchmark(&iteration_dir);
    assert_eq!(
        benchmark["run_summary"]["with_skill"]["total_tokens"]["n"],
        1
    );
    assert_eq!(
        benchmark["run_summary"]["with_skill"]["duration_ms"]["n"],
        0
    );
    assert_eq!(
        benchmark["run_summary"]["without_skill"]["total_tokens"]["n"],
        0
    );
    assert_eq!(
        benchmark["run_summary"]["without_skill"]["duration_ms"]["n"],
        0
    );
    let warnings = benchmark["validity_warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|warning| {
        let warning = warning.as_str().unwrap();
        warning.contains("condition 'with_skill'")
            && warning.contains("total_tokens: 1/1")
            && warning.contains("duration_ms: 0/1")
            && warning.contains("n: 0 is unavailable, not a measured zero")
    }));
    assert!(warnings.iter().any(|warning| {
        let warning = warning.as_str().unwrap();
        warning.contains("condition 'without_skill'")
            && warning.contains("total_tokens: 0/1")
            && warning.contains("duration_ms: 0/1")
    }));
}
