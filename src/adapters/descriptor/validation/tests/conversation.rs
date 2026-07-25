use super::{TOOLED, err_of, load_descriptor};

#[test]
fn accepts_conversation_resume_with_ordered_messages_and_session_id() {
    let src = format!(
        "{TOOLED}\n\
         [transcript]\n\
         events_filename = \"demo-events.jsonl\"\n\n\
         [transcript.extract.assistant_messages]\n\
         field = \"text\"\n\n\
         [transcript.extract.session_id]\n\
         field = \"session_id\"\n\n\
         [dispatch]\n\
         exec_template = \"demo <eval-root> <outputs_dir>\"\n\n\
         [conversation]\n\
         resume_exec_template = \"demo resume --cd <eval-root> {{session_arg}} \
         {{prompt_arg}} > <outputs_dir>/demo-events.jsonl\"\n"
    );
    load_descriptor(&src, "test.toml").expect("conversation resume capability should load");
}

#[test]
fn rejects_conversation_resume_without_required_placeholders() {
    let base = format!(
        "{TOOLED}\n\
         [transcript]\n\
         events_filename = \"demo-events.jsonl\"\n\
         parser = \"codex-items\"\n\n\
         [dispatch]\n\
         exec_template = \"demo <eval-root> <outputs_dir>\"\n\n\
         [conversation]\n\
         resume_exec_template = \"demo resume --cd <eval-root> {{session_arg}} \
         {{prompt_arg}} > <outputs_dir>/demo-events.jsonl\"\n"
    );
    for placeholder in [
        "<eval-root>",
        "<outputs_dir>",
        "{session_arg}",
        "{prompt_arg}",
    ] {
        let err = err_of(&base.replace(placeholder, ""));
        assert!(
            err.contains(placeholder),
            "expected {placeholder} in: {err}"
        );
    }
}

#[test]
fn rejects_declarative_conversation_resume_without_session_or_messages() {
    for omitted in ["assistant_messages", "session_id"] {
        let mut extract = String::new();
        if omitted != "assistant_messages" {
            extract.push_str("\n[transcript.extract.assistant_messages]\nfield = \"text\"\n");
        }
        if omitted != "session_id" {
            extract.push_str("\n[transcript.extract.session_id]\nfield = \"session_id\"\n");
        }
        let src = format!(
            "{TOOLED}\n\
             [transcript]\n\
             events_filename = \"demo-events.jsonl\"\n\
             {extract}\n\
             [dispatch]\n\
             exec_template = \"demo <eval-root> <outputs_dir>\"\n\n\
             [conversation]\n\
             resume_exec_template = \"demo resume --cd <eval-root> {{session_arg}} \
             {{prompt_arg}} > <outputs_dir>/demo-events.jsonl\"\n"
        );
        let err = err_of(&src);
        assert!(err.contains(omitted), "expected {omitted} in: {err}");
    }
}
