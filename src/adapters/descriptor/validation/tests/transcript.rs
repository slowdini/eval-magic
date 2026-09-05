use super::{MINIMAL, TOOLED, err_of, load_descriptor};

#[test]
fn rejects_transcript_without_write_and_shell_tools() {
    let err = err_of(&format!(
        "{MINIMAL}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\nparser = \"codex-items\"\n"
    ));
    assert!(err.contains("detect-stray-writes"), "{err}");
}

#[test]
fn rejects_extract_transcript_without_write_and_shell_tools() {
    let err = err_of(&format!(
        "{MINIMAL}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n\
         [transcript.extract.final_text]\nfield = \"text\"\n"
    ));
    assert!(err.contains("detect-stray-writes"), "{err}");
}

#[test]
fn rejects_transcript_with_parser_and_extract() {
    let err = err_of(&format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\nparser = \"codex-items\"\n\n\
         [transcript.extract.final_text]\nfield = \"text\"\n"
    ));
    assert!(err.contains("exactly one"), "{err}");
    assert!(err.contains("new label"), "{err}");
}

#[test]
fn accepts_session_surface_extract_alongside_a_named_parser() {
    let descriptor = format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\
         parser = \"claude-stream-json\"\n\n\
         [transcript.extract.session_surface]\n\
         where = {{ type = \"system\", subtype = \"init\" }}\n\
         skills_field = \"skills\"\n\
         plugins_field = \"plugins\"\n\
         plugin_name_field = \"name\"\n\
         plugin_id_field = \"source\"\n\
         plugin_version_field = \"version\"\n"
    );

    load_descriptor(&descriptor, "test.toml").expect("auxiliary extraction should compose");
}

#[test]
fn rejects_session_surface_without_a_primary_summary_reader() {
    let err = err_of(&format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n\
         [transcript.extract.session_surface]\n\
         skills_field = \"skills\"\n"
    ));

    assert!(err.contains("primary summary"), "{err}");
    assert!(err.contains("parser"), "{err}");
}

#[test]
fn rejects_session_surface_without_a_roster_field() {
    let err = err_of(&format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n\
         [transcript.extract.session_surface]\n\
         where = {{ type = \"system\", subtype = \"init\" }}\n"
    ));

    assert!(err.contains("skills_field"), "{err}");
    assert!(err.contains("plugins_field"), "{err}");
}

#[test]
fn rejects_plugin_roster_without_a_plugin_name_mapping() {
    let err = err_of(&format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n\
         [transcript.extract.session_surface]\n\
         plugins_field = \"plugins\"\n\
         plugin_id_field = \"source\"\n"
    ));

    assert!(err.contains("plugin_name_field"), "{err}");
    assert!(err.contains("plugins_field"), "{err}");
}

#[test]
fn rejects_plugin_mappings_without_a_plugin_roster() {
    for mapping in [
        "plugin_name_field = \"name\"",
        "plugin_id_field = \"source\"",
        "plugin_version_field = \"version\"",
    ] {
        let err = err_of(&format!(
            "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n\
             [transcript.extract.session_surface]\n\
             skills_field = \"skills\"\n\
             {mapping}\n"
        ));

        assert!(err.contains("plugins_field"), "{mapping}: {err}");
    }
}

#[test]
fn accepts_an_independent_permission_denials_parser_with_extract_ingest() {
    let descriptor = format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\
         permission_denials_parser = \"codex-items\"\n\n\
         [transcript.extract.final_text]\n\
         where = {{ type = \"item.completed\", \"item.type\" = \"agent_message\" }}\n\
         field = \"item.text\"\n"
    );

    load_descriptor(&descriptor, "test.toml").expect("denial parsing should compose");
}

#[test]
fn accepts_a_successful_exact_path_skill_access_signature() {
    let descriptor = format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\
         parser = \"codex-items\"\nsurfaces_skill_invocation = true\n\n\
         [transcript.skill_access]\n\
         tool = \"command_execution\"\n\
         command_arg = \"command\"\n\
         exit_code_arg = \"exit_code\"\n\
         read_commands = [\"cat\", \"sed\"]\n"
    );

    load_descriptor(&descriptor, "test.toml")
        .expect("an exact-path access signature should be descriptor-declared");
}

#[test]
fn rejects_skill_access_that_is_not_a_true_exclusive_shell_signal() {
    for (extra_transcript, expected) in [
        (
            "surfaces_skill_invocation = false\n",
            "surfaces_skill_invocation",
        ),
        (
            "surfaces_skill_invocation = true\nskill_tool = \"Skill\"\n",
            "skill_tool",
        ),
    ] {
        let err = err_of(&format!(
            "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\
             parser = \"codex-items\"\n{extra_transcript}\n\
             [transcript.skill_access]\n\
             tool = \"command_execution\"\n\
             command_arg = \"command\"\n\
             exit_code_arg = \"exit_code\"\n\
             read_commands = [\"cat\", \"sed\"]\n"
        ));
        assert!(err.contains(expected), "{err}");
    }

    let err = err_of(&format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\
         parser = \"codex-items\"\nsurfaces_skill_invocation = true\n\n\
         [transcript.skill_access]\n\
         tool = \"file_change\"\n\
         command_arg = \"command\"\n\
         exit_code_arg = \"exit_code\"\n\
         read_commands = [\"cat\", \"sed\"]\n"
    ));
    assert!(err.contains("tools.shell"), "{err}");
}

#[test]
fn rejects_skill_access_read_commands_that_are_not_literal_basenames() {
    let err = err_of(&format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\
         parser = \"codex-items\"\nsurfaces_skill_invocation = true\n\n\
         [transcript.skill_access]\n\
         tool = \"command_execution\"\n\
         command_arg = \"command\"\n\
         exit_code_arg = \"exit_code\"\n\
         read_commands = [\"/bin/cat\"]\n"
    ));
    assert!(err.contains("literal executable basenames"), "{err}");
}

#[test]
fn rejects_transcript_with_neither_parser_nor_extract() {
    for auxiliary in ["", "permission_denials_parser = \"codex-items\"\n"] {
        let err = err_of(&format!(
            "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n{auxiliary}"
        ));
        assert!(err.contains("exactly one"), "{err}");
        assert!(err.contains("llm_judge"), "{err}");
    }
}

#[test]
fn rejects_empty_extract_block() {
    let err = err_of(&format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n[transcript.extract]\n"
    ));
    assert!(err.contains("[transcript.extract]"), "{err}");
    assert!(err.contains("at least one"), "{err}");
}

#[test]
fn rejects_declarative_transcript_without_assistant_or_final_text() {
    let err = err_of(&format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n\
         [transcript.extract.tools]\nname_field = \"name\"\n"
    ));
    assert!(err.contains("final response"), "{err}");
    assert!(err.contains("final_text"), "{err}");
}

#[test]
fn rejects_duration_with_both_variants_or_neither() {
    for duration_block in [
        "field = \"elapsed_ms\"\ntimestamp_spread = \"timestamp\"\n",
        "",
    ] {
        let err = err_of(&format!(
            "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n\
             [transcript.extract.duration]\n{duration_block}"
        ));
        assert!(err.contains("duration"), "{err}");
        assert!(err.contains("exactly one"), "{err}");
    }
}
