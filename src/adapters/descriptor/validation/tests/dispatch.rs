use super::{MINIMAL, err_of};

#[test]
fn rejects_judge_template_without_model_flag() {
    let err = err_of(&format!(
        "{MINIMAL}\n[dispatch]\ncapture_prefix = \"demo\"\njudge_command_template = '    demo --cd \"{{cwd}}\" $model_arg \\'\n"
    ));
    assert!(err.contains("model.flag"), "{err}");
}

#[test]
fn rejects_judge_template_violating_the_recipe_contract() {
    for (template, needle) in [
        ("'    demo --cd \"{cwd}\" \\'", "$model_arg"),
        ("'    demo $model_arg \\'", "{cwd}"),
        ("'    demo --cd \"{cwd}\" $model_arg'", "line continuation"),
    ] {
        let err = err_of(&format!(
            "{MINIMAL}\n[model]\nflag = \"-m\"\n\n[dispatch]\ncapture_prefix = \"demo\"\njudge_command_template = {template}\n"
        ));
        assert!(err.contains(needle), "expected {needle} in: {err}");
    }
}

#[test]
fn rejects_judge_template_without_capture_prefix() {
    let err = err_of(&format!(
        "{MINIMAL}\n[model]\nflag = \"-m\"\n\n[dispatch]\njudge_command_template = '    demo --cd \"{{cwd}}\" $model_arg \\'\n"
    ));
    assert!(err.contains("capture_prefix"), "{err}");
}

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
        (
            "exec_template = \"demo run\"\nmanifest_template = \"use:\\n{exec_command}\\n{parallel_recipe}\\n\"",
            "{parallel_recipe}",
        ),
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
