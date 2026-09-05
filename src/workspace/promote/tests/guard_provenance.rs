use super::*;

#[test]
fn distinguishes_guard_state_from_legacy_unknown() {
    for (guard_armed, expected) in [
        (Some(true), "true"),
        (Some(false), "false"),
        (None, "unknown"),
    ] {
        let f = fixture(1);
        let mut conditions: Value = serde_json::from_str(CONDITIONS_WITH_PROVENANCE).unwrap();
        if let Some(guard_armed) = guard_armed {
            conditions["guard_armed"] = Value::Bool(guard_armed);
        }
        write(
            &f.iteration_dir.join("conditions.json"),
            &serde_json::to_string(&conditions).unwrap(),
        );
        write(
            &f.iteration_dir.join("benchmark.json"),
            r#"{"delta":{"pass_rate":0}}"#,
        );

        promote_baseline(&opts(&f, 1)).unwrap();

        let provenance =
            fs::read_to_string(f.skill_subdir.join("evals/baseline/BASELINE.md")).unwrap();
        assert!(
            provenance.contains(&format!("| Guard armed | {expected} |")),
            "guard_armed={guard_armed:?}: {provenance}"
        );
    }
}
