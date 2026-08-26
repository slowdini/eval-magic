use serde_json::{Value, json};

use super::validate_evals_config;

fn base() -> Value {
    json!({
        "skill_name": "demo",
        "codebase": { "path": "." },
        "evals": [{
            "id": "e1",
            "prompt": "do the thing",
            "expected_output": "the thing is done"
        }]
    })
}

#[test]
fn accepts_an_ordered_set_of_skills_under_test() {
    let mut config = base();
    config["skill_name"] = json!(["demo", "helper"]);

    let parsed = validate_evals_config(&config, "evals.json").unwrap();

    assert_eq!(parsed.skill_names(), ["demo", "helper"]);
}

#[test]
fn rejects_an_empty_or_duplicate_skill_set() {
    for names in [json!([]), json!(["demo", "demo"])] {
        let mut config = base();
        config["skill_name"] = names;

        assert!(validate_evals_config(&config, "evals.json").is_err());
    }
}
