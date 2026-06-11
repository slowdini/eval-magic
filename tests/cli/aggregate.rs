//! The `aggregate` subcommand — benchmark deltas and validity warnings.

use crate::helpers::{canonical_root, skill_eval};
use assert_cmd::Command;
use std::fs;

/// Create skill-dir/SKILL.md + iteration-1, returning
/// `(skill_dir, skill_md_path, iteration_dir, cwd)`.
fn setup_agg(
    root: &std::path::Path,
) -> (
    std::path::PathBuf,
    String,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    fs::create_dir_all(&skill_sub).unwrap();
    fs::write(
        skill_sub.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
    )
    .unwrap();
    let skill_md = skill_sub.join("SKILL.md").to_string_lossy().into_owned();
    let cwd = root.join("work");
    let iteration_dir = cwd
        .join("skills-workspace")
        .join("mr-review")
        .join("iteration-1");
    fs::create_dir_all(&iteration_dir).unwrap();
    (skill_dir, skill_md, iteration_dir, cwd)
}

/// Write `eval-e1/<cond>/grading.json` with the given pass rate.
fn write_grading(iteration_dir: &std::path::Path, cond: &str, pass_rate: f64) {
    write_grading_in(&iteration_dir.join("eval-e1").join(cond), pass_rate);
}

/// Write `<run_dir>/grading.json` with the given pass rate.
fn write_grading_in(run_dir: &std::path::Path, pass_rate: f64) {
    fs::create_dir_all(run_dir).unwrap();
    fs::write(
        run_dir.join("grading.json"),
        serde_json::to_string(&serde_json::json!({
            "assertion_results": [],
            "summary": {"passed": 1, "failed": 0, "total": 1, "pass_rate": pass_rate},
        }))
        .unwrap(),
    )
    .unwrap();
}

/// Write `eval-e1/<cond>/timing.json` (the cond dir must already exist).
fn write_timing(iteration_dir: &std::path::Path, cond: &str, timing: serde_json::Value) {
    write_timing_in(&iteration_dir.join("eval-e1").join(cond), timing);
}

/// Write `<run_dir>/timing.json` (the run dir must already exist).
fn write_timing_in(run_dir: &std::path::Path, timing: serde_json::Value) {
    fs::write(
        run_dir.join("timing.json"),
        serde_json::to_string(&timing).unwrap(),
    )
    .unwrap();
}

fn agg_cmd(cwd: &std::path::Path, skill_dir: &std::path::Path) -> Command {
    let mut cmd = skill_eval();
    cmd.current_dir(cwd)
        .arg("aggregate")
        .arg("--skill-dir")
        .arg(skill_dir)
        .arg("--skill")
        .arg("mr-review")
        .arg("--iteration")
        .arg("1");
    cmd
}

fn read_benchmark(iteration_dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(iteration_dir.join("benchmark.json")).unwrap())
        .unwrap()
}

/// Two new-skill conditions (`with_skill` loaded, `without_skill` null).
fn new_skill_conditions(iteration_dir: &std::path::Path, skill_md: &str) {
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&serde_json::json!({
            "mode": "new-skill",
            "conditions": [
                {"name": "with_skill", "skill_path": skill_md},
                {"name": "without_skill", "skill_path": null},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z",
            "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
}

/// `aggregate`: computes pass-rate means and the token delta from grading/timing.
#[test]
fn aggregate_computes_benchmark_from_graded_workspace() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    write_grading(&iteration_dir, "with_skill", 1.0);
    write_timing(
        &iteration_dir,
        "with_skill",
        serde_json::json!({"total_tokens": 5000, "duration_ms": 1000}),
    );
    write_grading(&iteration_dir, "without_skill", 0.0);
    write_timing(
        &iteration_dir,
        "without_skill",
        serde_json::json!({"total_tokens": 3000, "duration_ms": 1000}),
    );

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    assert_eq!(
        b["run_summary"]["with_skill"]["pass_rate"]["mean"]
            .as_f64()
            .unwrap(),
        1.0
    );
    assert_eq!(
        b["run_summary"]["without_skill"]["pass_rate"]["mean"]
            .as_f64()
            .unwrap(),
        0.0
    );
    assert_eq!(b["delta"]["pass_rate"].as_f64().unwrap(), 1.0);
    assert_eq!(b["delta"]["total_tokens"].as_f64().unwrap(), 2000.0);
}

/// `aggregate`: every `run-<k>` subdirectory of a condition cell contributes a
/// sample, so `n` reflects runs and the mean averages across them.
#[test]
fn aggregate_collects_all_runs_from_nested_run_dirs() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    for (cond, rates) in [("with_skill", [1.0, 0.5]), ("without_skill", [0.0, 0.5])] {
        for (k, rate) in rates.iter().enumerate() {
            let run_dir = iteration_dir
                .join("eval-e1")
                .join(cond)
                .join(format!("run-{}", k + 1));
            write_grading_in(&run_dir, *rate);
            write_timing_in(&run_dir, json!({"total_tokens": 1000, "duration_ms": 100}));
        }
    }

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    let with_skill = &b["run_summary"]["with_skill"]["pass_rate"];
    assert_eq!(with_skill["n"].as_u64().unwrap(), 2);
    assert_eq!(with_skill["mean"].as_f64().unwrap(), 0.75);
    let without_skill = &b["run_summary"]["without_skill"]["pass_rate"];
    assert_eq!(without_skill["n"].as_u64().unwrap(), 2);
    assert_eq!(without_skill["mean"].as_f64().unwrap(), 0.25);
    assert_eq!(b["delta"]["pass_rate"].as_f64().unwrap(), 0.5);
}

/// `aggregate`: uneven run counts across the two conditions weaken the delta,
/// so they surface as a validity warning.
#[test]
fn aggregate_warns_on_uneven_run_counts_across_conditions() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    for k in [1, 2] {
        let run_dir = iteration_dir
            .join("eval-e1")
            .join("with_skill")
            .join(format!("run-{k}"));
        write_grading_in(&run_dir, 1.0);
        write_timing_in(&run_dir, json!({"total_tokens": 1000, "duration_ms": 100}));
    }
    write_grading(&iteration_dir, "without_skill", 1.0);
    write_timing(
        &iteration_dir,
        "without_skill",
        json!({"total_tokens": 1000, "duration_ms": 100}),
    );

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    let warns = b["validity_warnings"].as_array().unwrap();
    assert!(
        warns
            .iter()
            .any(|w| w.as_str().unwrap().contains("uneven run counts")),
        "expected an uneven-run-counts warning, got: {warns:?}"
    );
}

/// `aggregate`: stray-write violations surface as validity_warnings.
#[test]
fn aggregate_surfaces_stray_write_violations() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
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
        iteration_dir.join("stray-writes.json"),
        serde_json::to_string(&json!({
            "generated": "2026-06-08T00:00:00.000Z", "iteration": 1,
            "totals": {"violations": 1, "warnings": 0},
            "runs": [{"eval_id": "e1", "condition": "with_skill",
                "violations": [{"tool": "Write", "path": "/repo/runner/run.ts", "ordinal": 3, "reason": "x"}],
                "warnings": []}],
        }))
        .unwrap(),
    )
    .unwrap();

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    let warns = b["validity_warnings"].as_array().unwrap();
    assert!(warns.iter().any(|w| {
        let s = w.as_str().unwrap();
        s.contains("e1/with_skill") && s.contains("outside")
    }));
}

/// `aggregate`: live-source reads surface as validity_warnings.
#[test]
fn aggregate_surfaces_live_source_reads() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&json!({
            "mode": "revision",
            "conditions": [
                {"name": "old_skill", "skill_path": skill_md},
                {"name": "new_skill", "skill_path": skill_md},
            ],
            "timestamp": "2026-06-08T00:00:00.000Z", "harness": "claude-code",
        }))
        .unwrap(),
    )
    .unwrap();
    for cond in ["old_skill", "new_skill"] {
        write_grading(&iteration_dir, cond, 1.0);
        write_timing(
            &iteration_dir,
            cond,
            json!({"total_tokens": 100, "duration_ms": 1}),
        );
    }
    fs::write(
        iteration_dir.join("stray-writes.json"),
        serde_json::to_string(&json!({
            "generated": "2026-06-08T00:00:00.000Z", "iteration": 1,
            "totals": {"violations": 0, "warnings": 0, "live_source_reads": 1},
            "runs": [{"eval_id": "e1", "condition": "old_skill", "violations": [], "warnings": [],
                "live_source_reads": [{"tool": "Read", "path": skill_md, "ordinal": 0, "reason": "x"}]}],
        }))
        .unwrap(),
    )
    .unwrap();

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    let warns = b["validity_warnings"].as_array().unwrap();
    assert!(warns.iter().any(|w| {
        let s = w.as_str().unwrap();
        s.contains("e1/old_skill") && s.to_lowercase().contains("live skill source")
    }));
}

/// `aggregate`: warns when timing sources are mixed across the compared runs.
#[test]
fn aggregate_warns_on_mixed_timing_sources() {
    use serde_json::json;
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
        json!({"total_tokens": 90000, "duration_ms": 1200, "source": "transcript"}),
    );

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    let warns = b["validity_warnings"].as_array().unwrap();
    assert!(warns.iter().any(|w| {
        let s = w.as_str().unwrap();
        s.contains("timing source") && s.contains("transcript")
    }));
}

/// `aggregate`: no timing-source warning when all runs share one source.
#[test]
fn aggregate_no_warning_when_timing_sources_match() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    for cond in ["with_skill", "without_skill"] {
        write_grading(&iteration_dir, cond, 1.0);
        write_timing(
            &iteration_dir,
            cond,
            json!({"total_tokens": 100, "duration_ms": 1, "source": "transcript"}),
        );
    }

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    let warns = b["validity_warnings"].as_array().unwrap();
    assert!(
        !warns
            .iter()
            .any(|w| w.as_str().unwrap().contains("timing source"))
    );
}

/// `aggregate`: plugin-shadow findings surface as validity_warnings.
#[test]
fn aggregate_surfaces_plugin_shadow_findings() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
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
        serde_json::to_string(&json!({
            "config_dir": "/home/u/.claude",
            "shadowed": [{"kind": "plugin", "plugin": "slow-powers@slowdini", "skill_name": "mr-review",
                "path": "/home/u/.claude/plugins/cache/slowdini/slow-powers/skills/mr-review"}],
        }))
        .unwrap(),
    )
    .unwrap();

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    let warns = b["validity_warnings"].as_array().unwrap();
    assert!(warns.iter().any(|w| {
        let s = w.as_str().unwrap();
        s.contains("mr-review") && s.to_lowercase().contains("contaminat")
    }));
}
