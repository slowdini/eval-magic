//! `[plan_mode]` invariants: the table needs `[conversation]` (the approved
//! plan is implemented by resuming the same session) and a `{mode_args}` slot
//! in both command templates, and the slot needs the table to fill it.

use super::{TOOLED, err_of, load_descriptor};

/// A runner-ready, resumable descriptor whose templates carry `{mode_args}`.
fn resumable(plan_mode: &str, exec_slot: &str, resume_slot: &str) -> String {
    format!(
        "{TOOLED}\n\
         [transcript]\n\
         events_filename = \"demo-events.jsonl\"\n\
         parser = \"codex-items\"\n\n\
         [dispatch]\n\
         exec_template = \"demo{exec_slot} <eval-root> <outputs_dir>\"\n\n\
         [conversation]\n\
         resume_exec_template = \"demo resume{resume_slot} --cd <eval-root> {{session_arg}} \
         {{prompt_arg}} > <outputs_dir>/demo-events.jsonl\"\n\
         {plan_mode}"
    )
}

const PLAN_MODE: &str = "\n[plan_mode]\nplan_args = \" --plan\"\nact_args = \" --act\"\n";

#[test]
fn accepts_plan_mode_with_mode_args_in_both_templates() {
    let src = resumable(PLAN_MODE, "{mode_args}", "{mode_args}");
    let descriptor = load_descriptor(&src, "test.toml").expect("plan mode should load");
    let plan_mode = descriptor.plan_mode.expect("[plan_mode] is kept");
    assert_eq!(plan_mode.plan_args, " --plan");
    assert_eq!(plan_mode.act_args, " --act");
    assert!(plan_mode.plan_file.is_none());
}

#[test]
fn accepts_a_plan_file_declaration() {
    let plan_mode = format!(
        "{PLAN_MODE}\n[plan_mode.plan_file]\nroot = \"~/.demo/plans\"\ncontent_field = \"content\"\n"
    );
    let src = resumable(&plan_mode, "{mode_args}", "{mode_args}");
    let descriptor = load_descriptor(&src, "test.toml").expect("plan file should load");
    let plan_file = descriptor
        .plan_mode
        .and_then(|plan_mode| plan_mode.plan_file)
        .expect("[plan_mode.plan_file] is kept");
    assert_eq!(plan_file.root, "~/.demo/plans");
    assert_eq!(plan_file.content_field, "content");
}

#[test]
fn rejects_plan_mode_without_conversation() {
    let src = format!(
        "{TOOLED}\n\
         [transcript]\n\
         events_filename = \"demo-events.jsonl\"\n\
         parser = \"codex-items\"\n\n\
         [dispatch]\n\
         exec_template = \"demo{{mode_args}} <eval-root> <outputs_dir>\"\n\
         {PLAN_MODE}"
    );
    let err = err_of(&src);
    assert!(
        err.contains("[plan_mode]") && err.contains("[conversation]"),
        "{err}"
    );
}

#[test]
fn rejects_plan_mode_when_a_template_lacks_mode_args() {
    let err = err_of(&resumable(PLAN_MODE, "", "{mode_args}"));
    assert!(
        err.contains("dispatch.exec_template") && err.contains("{mode_args}"),
        "{err}"
    );
    let err = err_of(&resumable(PLAN_MODE, "{mode_args}", ""));
    assert!(
        err.contains("conversation.resume_exec_template") && err.contains("{mode_args}"),
        "{err}"
    );
}

#[test]
fn rejects_mode_args_without_a_plan_mode_table() {
    let err = err_of(&resumable("", "{mode_args}", ""));
    assert!(
        err.contains("exec_template") && err.contains("{mode_args}") && err.contains("[plan_mode]"),
        "{err}"
    );
    let err = err_of(&resumable("", "", "{mode_args}"));
    assert!(
        err.contains("resume_exec_template")
            && err.contains("{mode_args}")
            && err.contains("[plan_mode]"),
        "{err}"
    );
}

#[test]
fn rejects_plan_mode_missing_a_required_field_at_the_schema_gate() {
    let src = resumable(
        "\n[plan_mode]\nplan_args = \" --plan\"\n",
        "{mode_args}",
        "{mode_args}",
    );
    let err = err_of(&src);
    assert!(err.contains("act_args"), "{err}");
}
