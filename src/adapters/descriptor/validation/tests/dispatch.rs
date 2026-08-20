use super::{MINIMAL, err_of};

#[test]
fn rejects_template_placeholders_without_backing_fields() {
    for (dispatch_body, needle) in [
        (
            "next_steps_template = \"do {exec_command} now\"",
            "{exec_command}",
        ),
        (
            "next_steps_template = \"go.{model_note} then\"",
            "{model_note}",
        ),
        ("exec_template = \"demo{guard_args} run\"", "{guard_args}"),
    ] {
        let err = err_of(&format!("{MINIMAL}\n[dispatch]\n{dispatch_body}\n"));
        assert!(err.contains(needle), "expected {needle} in: {err}");
    }
}

#[test]
fn rejects_manifest_template_without_single_trailing_newline() {
    for manifest in [
        "\"use:\\n{exec_command}\"",
        "\"use:\\n{exec_command}\\n\\n\"",
    ] {
        let err = err_of(&format!(
            "{MINIMAL}\n[dispatch]\nexec_template = \"demo run\"\nmanifest_template = {manifest}\n"
        ));
        assert!(err.contains("exactly one trailing newline"), "{err}");
    }
}

#[test]
fn rejects_skills_block_item_without_name_placeholder() {
    let err = err_of(&format!(
        "{MINIMAL}\n[skills_block]\nheader = \"Skills:\"\nitem = \"- {{description}}\"\n"
    ));
    assert!(err.contains("{name}"), "{err}");
}
