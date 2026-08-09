use super::*;
use crate::adapters::capabilities::TranscriptParser;
use crate::adapters::extract::ExtractSpec;

fn load(toml_src: &str) -> Result<HarnessDescriptor, DescriptorError> {
    load_descriptor(toml_src, "test.toml")
}

#[test]
fn transcript_section_prefers_an_explicit_session_surface_extractor() {
    let transcript = load(
        r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "demo-events.jsonl"
parser = "codex-items"

[transcript.extract.session_surface]
where = { kind = "roster" }
skills_field = "surface.skills"
"#,
    )
    .unwrap()
    .transcript
    .unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("demo-events.jsonl");
    std::fs::write(
        &path,
        "{\"kind\":\"roster\",\"surface\":{\"skills\":[\"demo:one\"]}}\n",
    )
    .unwrap();

    assert!(transcript.surfaces_session_surface());
    assert_eq!(
        transcript
            .parse_session_surface(&path)
            .unwrap()
            .unwrap()
            .advertised_skills,
        vec!["demo:one"]
    );
}

#[test]
fn explicit_session_surface_unavailability_does_not_fall_back_to_the_parser() {
    let transcript = load(
        r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "demo-events.jsonl"
parser = "claude-stream-json"

[transcript.extract.session_surface]
where = { type = "future-roster" }
skills_field = "skills"
"#,
    )
    .unwrap()
    .transcript
    .unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("demo-events.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"system\",\"subtype\":\"init\",\"skills\":[\"legacy\"]}\n",
    )
    .unwrap();

    assert_eq!(transcript.parse_session_surface(&path).unwrap(), None);
}

#[test]
fn parser_only_descriptor_retains_legacy_session_surface_support() {
    let transcript = load(
        r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[tools]
write = ["Edit"]
shell = ["Bash"]

[transcript]
events_filename = "demo-events.jsonl"
parser = "claude-stream-json"
"#,
    )
    .unwrap()
    .transcript
    .unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("demo-events.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"system\",\"subtype\":\"init\",\"skills\":[\"legacy\"],\"plugins\":[]}\n",
    )
    .unwrap();

    assert!(transcript.surfaces_session_surface());
    assert_eq!(
        transcript
            .parse_session_surface(&path)
            .unwrap()
            .unwrap()
            .advertised_skills,
        vec!["legacy"]
    );
}

#[test]
fn transcript_section_routes_denials_through_the_independent_parser() {
    let transcript = load(
        r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "demo-events.jsonl"
permission_denials_parser = "codex-items"

[transcript.extract.final_text]
field = "text"
"#,
    )
    .unwrap()
    .transcript
    .unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("demo-events.jsonl");
    std::fs::write(&path, "{\"text\":\"done\"}\n").unwrap();

    assert!(transcript.surfaces_permission_denials());
    assert_eq!(transcript.parse_permission_denials(&path).unwrap(), vec![]);
}

#[test]
fn builtins_declare_composed_transcript_capabilities() {
    let load_builtin = |label: &str| {
        let (path, source) = EMBEDDED_DESCRIPTORS
            .iter()
            .find(|(_, source)| source.contains(&format!("label = \"{label}\"")))
            .unwrap();
        load_descriptor(source, path).unwrap()
    };

    let claude = load_builtin("claude-code").transcript.unwrap();
    assert_eq!(
        claude.permission_denials_parser,
        Some(TranscriptParser::ClaudeStreamJson)
    );
    assert!(
        claude
            .extract
            .as_ref()
            .and_then(|extract| extract.session_surface.as_ref())
            .is_some()
    );

    let codex = load_builtin("codex").transcript.unwrap();
    assert!(codex.parser.is_none());
    assert!(
        codex
            .extract
            .as_ref()
            .is_some_and(ExtractSpec::has_summary_outputs)
    );
    assert_eq!(
        codex.permission_denials_parser,
        Some(TranscriptParser::CodexItems)
    );

    let opencode = load_builtin("opencode").transcript.unwrap();
    assert_eq!(
        opencode.permission_denials_parser,
        Some(TranscriptParser::OpencodeEvents)
    );
}
