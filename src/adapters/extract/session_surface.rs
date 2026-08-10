use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;
use std::path::Path;

use crate::adapters::transcript::read_jsonl;
use crate::adapters::{LoadedPlugin, SessionSurface};

use super::{Where, matches, resolve};

/// Declarative mapping from one matching transcript record to the session's
/// advertised skill and loaded-plugin rosters.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionSurfaceExtract {
    #[serde(default, rename = "where", skip_serializing_if = "Where::is_empty")]
    pub r#where: Where,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_name_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_version_field: Option<String>,
}

/// Extract the last matching record that positively reports at least one of
/// the configured roster arrays.
pub(crate) fn parse(
    spec: &SessionSurfaceExtract,
    path: &Path,
) -> io::Result<Option<SessionSurface>> {
    let records = read_jsonl::<Value>(path)?;
    Ok(records
        .iter()
        .filter(|record| matches(record, &spec.r#where))
        .filter_map(|record| extract_record(spec, record))
        .next_back())
}

fn extract_record(spec: &SessionSurfaceExtract, record: &Value) -> Option<SessionSurface> {
    let skills = spec
        .skills_field
        .as_deref()
        .and_then(|field| resolve(record, field))
        .and_then(Value::as_array);
    let plugins = spec
        .plugins_field
        .as_deref()
        .and_then(|field| resolve(record, field))
        .and_then(Value::as_array);
    if skills.is_none() && plugins.is_none() {
        return None;
    }

    Some(SessionSurface {
        advertised_skills: skills
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        loaded_plugins: plugins
            .into_iter()
            .flatten()
            .filter_map(|plugin| extract_plugin(spec, plugin))
            .collect(),
    })
}

fn extract_plugin(spec: &SessionSurfaceExtract, value: &Value) -> Option<LoadedPlugin> {
    if let Some(name) = value.as_str() {
        return Some(LoadedPlugin {
            name: name.to_string(),
            source: None,
            version: None,
        });
    }
    value.as_object()?;
    let name = spec
        .plugin_name_field
        .as_deref()
        .and_then(|field| resolve(value, field))
        .and_then(Value::as_str)?;
    let mapped_string = |field: Option<&str>| {
        field
            .and_then(|field| resolve(value, field))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    Some(LoadedPlugin {
        name: name.to_string(),
        source: mapped_string(spec.plugin_id_field.as_deref()),
        version: mapped_string(spec.plugin_version_field.as_deref()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn spec() -> SessionSurfaceExtract {
        toml::from_str(
            r#"
            where = { type = "system", subtype = "init" }
            skills_field = "skills"
            plugins_field = "plugins"
            plugin_name_field = "name"
            plugin_id_field = "source"
            plugin_version_field = "version"
            "#,
        )
        .unwrap()
    }

    fn write_jsonl(path: &Path, lines: &[Value]) {
        let body = lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{body}\n")).unwrap();
    }

    #[test]
    fn last_matching_roster_preserves_plugin_identity_and_skips_malformed_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        write_jsonl(
            &path,
            &[
                json!({"type": "system", "subtype": "init", "skills": ["old"], "plugins": []}),
                json!({"type": "system", "subtype": "hook_started"}),
                json!({
                    "type": "system",
                    "subtype": "init",
                    "skills": ["plain", 7, "slow-powers:hardening-plans"],
                    "plugins": [
                        {"name": "slow-powers", "source": "slow-powers@slowdini", "version": "0.5.2"},
                        "context7",
                        {"name": 7},
                        false
                    ]
                }),
                json!({"type": "system", "subtype": "init", "session_id": "no-roster"}),
            ],
        );

        let surface = parse(&spec(), &path).unwrap().unwrap();
        assert_eq!(
            surface.advertised_skills,
            vec!["plain", "slow-powers:hardening-plans"]
        );
        assert_eq!(surface.loaded_plugins.len(), 2);
        assert_eq!(surface.loaded_plugins[0].name, "slow-powers");
        assert_eq!(
            surface.loaded_plugins[0].source.as_deref(),
            Some("slow-powers@slowdini")
        );
        assert_eq!(surface.loaded_plugins[0].version.as_deref(), Some("0.5.2"));
        assert_eq!(surface.loaded_plugins[1].name, "context7");
        assert_eq!(surface.loaded_plugins[1].source, None);
        assert_eq!(surface.loaded_plugins[1].version, None);
    }

    #[test]
    fn missing_rosters_are_none_while_explicit_empty_arrays_are_evidence() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing.jsonl");
        write_jsonl(
            &missing,
            &[json!({"type": "system", "subtype": "init", "skills": "unknown"})],
        );
        assert_eq!(parse(&spec(), &missing).unwrap(), None);

        let empty = dir.path().join("empty.jsonl");
        write_jsonl(
            &empty,
            &[json!({"type": "system", "subtype": "init", "skills": [], "plugins": []})],
        );
        let surface = parse(&spec(), &empty).unwrap().unwrap();
        assert!(surface.advertised_skills.is_empty());
        assert!(surface.loaded_plugins.is_empty());
    }

    #[test]
    fn declarative_claude_spec_matches_the_named_reference_parser() {
        use crate::adapters::claude_code::stream_json::parse_claude_session_surface;

        let corpora = [
            vec![json!({
                "type": "system",
                "subtype": "init",
                "skills": ["slow-powers:hardening-plans"],
                "plugins": [{
                    "name": "slow-powers",
                    "source": "slow-powers@slowdini",
                    "version": "0.5.2"
                }]
            })],
            vec![json!({
                "type": "system",
                "subtype": "init",
                "skills": [],
                "plugins": []
            })],
            vec![json!({"type": "result", "subtype": "success"})],
            vec![
                json!({
                    "type": "system",
                    "subtype": "init",
                    "skills": ["reported"],
                    "plugins": []
                }),
                json!({"type": "system", "subtype": "init", "session_id": "no-roster"}),
            ],
        ];
        let dir = TempDir::new().unwrap();
        for (index, corpus) in corpora.iter().enumerate() {
            let path = dir.path().join(format!("corpus-{index}.jsonl"));
            write_jsonl(&path, corpus);
            assert_eq!(
                parse(&spec(), &path).unwrap(),
                parse_claude_session_surface(&path).unwrap(),
                "parsers diverged on corpus {index}"
            );
        }
    }
}
