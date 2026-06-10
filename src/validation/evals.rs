//! High-level `evals.json` validation: structural schema check plus the
//! hand-rolled constraints draft-07 can't express.

use std::collections::HashSet;

use serde_json::Value;

use crate::core::EvalsConfig;
use crate::validation::error::ValidationError;
use crate::validation::schema::{SchemaName, validate_against_schema};

/// Validate a parsed `evals.json`. Runs the structural schema check, then the
/// supplemental duplicate-`id` guard (uniqueness by a sub-field isn't
/// expressible in JSON Schema draft-07), returning the typed config on success.
pub fn validate_evals_config(config: &Value, source: &str) -> Result<EvalsConfig, ValidationError> {
    let validated: EvalsConfig = validate_against_schema(SchemaName::Evals, config, source)?;

    let mut seen = HashSet::new();
    for (index, ev) in validated.evals.iter().enumerate() {
        if !seen.insert(ev.id.as_str()) {
            return Err(ValidationError::DuplicateId {
                path: source.to_string(),
                index,
                id: ev.id.clone(),
            });
        }
    }

    Ok(validated)
}

#[cfg(test)]
mod tests {
    use super::validate_evals_config;
    use serde_json::{Value, json};

    /// The minimal valid config the cases below mutate.
    fn base() -> Value {
        json!({
            "skill_name": "demo",
            "evals": [
                {
                    "id": "e1",
                    "prompt": "do the thing",
                    "expected_output": "the thing is done"
                }
            ]
        })
    }

    #[test]
    fn accepts_a_boolean_skill_should_trigger() {
        let mut config = base();
        config["evals"][0]["skill_should_trigger"] = json!(false);
        let parsed = validate_evals_config(&config, "evals.json").unwrap();
        assert_eq!(parsed.evals[0].skill_should_trigger, Some(false));
    }

    #[test]
    fn accepts_evals_with_no_skill_should_trigger() {
        let config = base();
        let parsed = validate_evals_config(&config, "evals.json").unwrap();
        assert_eq!(parsed.skill_name, "demo");
        assert_eq!(parsed.evals[0].skill_should_trigger, None);
    }

    #[test]
    fn rejects_a_non_boolean_skill_should_trigger() {
        let mut config = base();
        config["evals"][0]["skill_should_trigger"] = json!("false");
        let err = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("skill_should_trigger"), "error was: {err}");
    }

    #[test]
    fn rejects_a_non_kebab_case_id() {
        let mut config = base();
        config["evals"][0]["id"] = json!("Not Kebab");
        assert!(validate_evals_config(&config, "evals.json").is_err());
    }

    #[test]
    fn rejects_duplicate_eval_ids() {
        let mut config = base();
        let dup = config["evals"][0].clone();
        config["evals"] = json!([dup.clone(), dup]);
        let err = validate_evals_config(&config, "evals.json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate"), "error was: {err}");
    }

    #[test]
    fn rejects_an_empty_evals_array() {
        let mut config = base();
        config["evals"] = json!([]);
        assert!(validate_evals_config(&config, "evals.json").is_err());
    }
}
