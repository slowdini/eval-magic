use super::*;

#[test]
fn dispatch_task_serializes_its_frozen_guard_policy() {
    let mut task = build_dispatch_task(&base_opts()).unwrap();
    task.guard_policy.allow_commands = vec!["cargo test".to_string()];

    let value = serde_json::to_value(task).unwrap();

    assert_eq!(
        value["guard_policy"]["allow_commands"],
        serde_json::json!(["cargo test"])
    );
}
