//! `aggregate` reading the verified shadow verdict.
//!
//! The build-time preflight reports what is *discoverable*; these cover what
//! `aggregate` does once transcripts have settled whether it actually loaded.

use super::*;
use serde_json::json;
use std::path::{Path, PathBuf};

/// A v2 shadow artifact for one subject collision, with `resolved_severity` and
/// per-source verification already applied — the shape `ingest` writes.
fn verified_artifact(resolved: &str, status: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut artifact = json!({
        "schema_version": 2,
        "config_dir": "/home/u/.claude",
        "findings": [{
            "skill_name": "mr-review",
            "role": "subject",
            "severity": "comparison-invalid",
            "resolved_severity": resolved,
            "sources": [{
                "kind": "plugin",
                "origin": "live",
                "skill_name": "mr-review",
                "runtime_id": "slow-powers:mr-review",
                "plugin": "slow-powers@slowdini",
                "discovery_path": "/home/u/.claude/plugins/cache/s/skills/mr-review",
                "root": {
                    "scope": "global",
                    "namespace": "plugin",
                    "plugin": "slow-powers@slowdini",
                    "path": "/home/u/.claude/plugins/cache/s/skills",
                    "relation": "native"
                },
                "appearances": [{
                    "group": "g1", "condition": "with_skill",
                    "eval_ids": ["e1"], "resolution": "selected"
                }],
                "remediation": "Disable plugin 'slow-powers@slowdini' for every dispatch.",
                "verification": {
                    "status": status,
                    "cells": [{
                        "group": "g1", "condition": "with_skill", "status": status,
                        "dispatches_with_evidence": 2, "dispatches_without_evidence": 0
                    }]
                }
            }]
        }]
    });
    if let Some(object) = extra.as_object() {
        for (key, value) in object {
            artifact[key] = value.clone();
        }
    }
    artifact
}

/// Set up an iteration with gradings, timing, and the given shadow artifact.
fn iteration_with(artifact: &serde_json::Value) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let (tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    for cond in ["with_skill", "without_skill"] {
        write_grading(&iteration_dir, cond, 1.0);
        write_timing(
            &iteration_dir,
            cond,
            json!({"total_tokens": 100, "duration_ms": 1}),
        );
    }
    fs::write(
        iteration_dir.join("plugin-shadow.json"),
        serde_json::to_string(artifact).unwrap(),
    )
    .unwrap();
    (tmp, skill_dir, iteration_dir, cwd)
}

fn shadow_warnings(iteration_dir: &Path) -> Vec<String> {
    read_benchmark(iteration_dir)["validity_warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_string())
        .filter(|w| w.contains("mr-review") || w.contains("isolates_live_sources"))
        .collect()
}

/// The issue #207 fix: evidence showed nothing loaded, so nothing is reported.
#[test]
fn aggregate_drops_a_shadow_warning_refuted_by_dispatch_evidence() {
    let (_tmp, skill_dir, iteration_dir, cwd) =
        iteration_with(&verified_artifact("isolated", "refuted", json!({})));

    agg_cmd(&cwd, &skill_dir).assert().success();

    assert!(
        shadow_warnings(&iteration_dir).is_empty(),
        "a refuted finding is provenance, not a threat: {:?}",
        shadow_warnings(&iteration_dir)
    );
}

/// Evidence confirmed it: the verdict stands, and now names its cells.
#[test]
fn aggregate_keeps_comparison_invalid_when_evidence_confirms_the_collision() {
    let (_tmp, skill_dir, iteration_dir, cwd) = iteration_with(&verified_artifact(
        "comparison-invalid",
        "confirmed",
        json!({}),
    ));

    agg_cmd(&cwd, &skill_dir).assert().success();

    let warnings = shadow_warnings(&iteration_dir);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("comparison invalid"), "{warnings:?}");
    assert!(warnings[0].contains("was actually loaded"), "{warnings:?}");
    assert!(warnings[0].contains("g1/with_skill"), "{warnings:?}");
}

/// No usable transcript: the warning stays, labelled as unverified rather than
/// asserted — the operator needs to know which of the two it is.
#[test]
fn aggregate_keeps_the_warning_when_a_cell_has_no_transcript_evidence() {
    let mut artifact = verified_artifact("comparison-invalid", "unverified", json!({}));
    artifact["findings"][0]["sources"][0]["verification"]["cells"][0]["inconclusive_reason"] =
        json!("1 of 1 dispatch(es) reported no skill/plugin surface");
    let (_tmp, skill_dir, iteration_dir, cwd) = iteration_with(&artifact);

    agg_cmd(&cwd, &skill_dir).assert().success();

    let warnings = shadow_warnings(&iteration_dir);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("(unverified)"), "{warnings:?}");
    assert!(
        warnings[0].contains("reported no skill/plugin surface"),
        "names why it could not be settled: {warnings:?}"
    );
}

/// A declared isolation that evidence contradicts: the suppressed findings were
/// real, so the contradiction is reported rather than trusted.
#[test]
fn aggregate_reports_a_declared_isolation_assertion_contradicted_by_evidence() {
    let artifact = verified_artifact(
        "comparison-invalid",
        "confirmed",
        json!({
            "isolates_live_sources": true,
            "verification": {
                "generated": "2026-08-07T00:00:00Z",
                "harness_reports_session_surface": true,
                "dispatches_with_evidence": 2,
                "dispatches_without_evidence": 0,
                "refuted_findings": 0,
                "confirmed_findings": 1,
                "unverified_findings": 0,
                "assertion_contradicted": true
            }
        }),
    );
    let (_tmp, skill_dir, iteration_dir, cwd) = iteration_with(&artifact);

    agg_cmd(&cwd, &skill_dir).assert().success();

    let warnings = shadow_warnings(&iteration_dir);
    assert!(
        warnings.iter().any(|w| w.contains("isolates_live_sources")
            && w.contains("the isolation assertion is false")),
        "a contradicted assertion must be surfaced: {warnings:?}"
    );
}

/// A truthful assertion with no contradicting evidence still suppresses.
#[test]
fn aggregate_still_honors_an_uncontradicted_isolation_assertion() {
    let artifact = verified_artifact(
        "isolated",
        "refuted",
        json!({"isolates_live_sources": true}),
    );
    let (_tmp, skill_dir, iteration_dir, cwd) = iteration_with(&artifact);

    agg_cmd(&cwd, &skill_dir).assert().success();

    assert!(shadow_warnings(&iteration_dir).is_empty());
}

/// Every new warning string is self-contained: shipped output cites the embedded
/// topic, never a repo path a binary-only install cannot open.
#[test]
fn no_verified_shadow_warning_points_at_a_repo_path() {
    for (resolved, status) in [
        ("comparison-invalid", "confirmed"),
        ("comparison-invalid", "unverified"),
    ] {
        let (_tmp, skill_dir, iteration_dir, cwd) =
            iteration_with(&verified_artifact(resolved, status, json!({})));
        agg_cmd(&cwd, &skill_dir).assert().success();
        for warning in shadow_warnings(&iteration_dir) {
            assert!(!warning.contains("docs/"), "{warning}");
            assert!(
                warning.contains("eval-magic docs isolation"),
                "points at the embedded topic: {warning}"
            );
        }
    }
}
