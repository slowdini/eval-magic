use serde_json::{Value, json};

use super::evals::validate_evals_config;

fn base() -> Value {
    json!({
        "skill_name": "demo",
        "evals": [{
            "id": "e1",
            "prompt": "do the thing",
            "expected_output": "the thing is done"
        }]
    })
}

#[test]
fn eval_guard_policy_replaces_the_config_default() {
    let mut config = base();
    config["guard"] = json!({
        "profiles": ["language/rust"],
        "allow_commands": ["cargo test"]
    });
    config["evals"][0]["guard"] = json!({
        "allow_tools": ["cargo"],
        "allow_commands": ["npm run dev"]
    });

    let parsed = validate_evals_config(&config, "evals.json").unwrap();
    let policy = parsed.guard_for(&parsed.evals[0]).unwrap();

    assert!(policy.profiles.is_empty());
    assert_eq!(policy.allow_tools, ["cargo"]);
    assert_eq!(policy.allow_commands, ["npm run dev"]);
}

#[test]
fn rejects_unknown_guard_profiles() {
    let mut config = base();
    config["guard"] = json!({ "profiles": ["framework/imaginary"] });

    let error = validate_evals_config(&config, "evals.json")
        .unwrap_err()
        .to_string();

    assert!(error.contains("framework/imaginary"), "error was: {error}");
}

#[test]
fn rejects_dynamic_or_compound_guard_command_rules() {
    for rule in ["cargo $ACTION", "npm test && curl example.com"] {
        let mut config = base();
        config["evals"][0]["guard"] = json!({ "allow_commands": [rule] });

        let error = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("allow_commands"),
            "error for {rule:?}: {error}"
        );
    }
}
