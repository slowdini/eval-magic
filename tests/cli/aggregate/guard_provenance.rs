use super::*;

fn aggregate_with_guard_state(guard_armed: Option<bool>) -> serde_json::Value {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    if let Some(guard_armed) = guard_armed {
        let conditions_path = iteration_dir.join("conditions.json");
        let mut conditions = serde_json::from_str::<serde_json::Value>(
            &fs::read_to_string(&conditions_path).unwrap(),
        )
        .unwrap();
        conditions["guard_armed"] = serde_json::json!(guard_armed);
        fs::write(
            &conditions_path,
            serde_json::to_string(&conditions).unwrap(),
        )
        .unwrap();
    }
    for condition in ["with_skill", "without_skill"] {
        write_grading(&iteration_dir, condition, 1.0);
        write_timing(
            &iteration_dir,
            condition,
            serde_json::json!({"total_tokens": 100, "duration_ms": 1}),
        );
    }

    agg_cmd(&cwd, &skill_dir).assert().success();

    read_benchmark(&iteration_dir)
}

#[test]
fn echoes_effective_guard_provenance() {
    for guard_armed in [true, false] {
        assert_eq!(
            aggregate_with_guard_state(Some(guard_armed))["guard_armed"],
            guard_armed
        );
    }
}

#[test]
fn leaves_historical_guard_state_unknown() {
    let benchmark = aggregate_with_guard_state(None);
    assert!(
        benchmark.get("guard_armed").is_none(),
        "legacy conditions must not be rewritten as unguarded: {benchmark}"
    );
}
