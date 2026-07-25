use super::super::HarnessDescriptor;

pub(super) fn validate(descriptor: &HarnessDescriptor) -> Result<(), String> {
    let Some(conversation) = &descriptor.conversation else {
        return Ok(());
    };
    if descriptor.dispatch.exec_template.is_none() {
        return Err(
            "[conversation] requires dispatch.exec_template so the driver can start the \
             native session before resuming it"
                .into(),
        );
    }
    let Some(transcript) = &descriptor.transcript else {
        return Err(
            "[conversation] requires [transcript] so the driver can recover the native \
             session id and ordered assistant messages"
                .into(),
        );
    };
    if let Some(extract) = &transcript.extract {
        if extract.assistant_messages.is_none() {
            return Err(
                "[conversation] with declarative transcript extraction requires \
                 transcript.extract.assistant_messages"
                    .into(),
            );
        }
        if extract.session_id.is_none() {
            return Err(
                "[conversation] with declarative transcript extraction requires \
                 transcript.extract.session_id"
                    .into(),
            );
        }
    }
    for placeholder in [
        "<eval-root>",
        "<outputs_dir>",
        "{session_arg}",
        "{prompt_arg}",
    ] {
        if !conversation.resume_exec_template.contains(placeholder) {
            return Err(format!(
                "conversation.resume_exec_template must reference {placeholder}"
            ));
        }
    }
    if conversation.resume_exec_template.contains("{guard_args}")
        && descriptor.dispatch.guard_args.is_none()
    {
        return Err(
            "conversation.resume_exec_template references {guard_args} but \
             dispatch.guard_args is not set"
                .into(),
        );
    }
    Ok(())
}
