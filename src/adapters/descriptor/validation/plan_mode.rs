use super::super::HarnessDescriptor;

const SLOT: &str = "{mode_args}";

/// `[plan_mode]` fills the `{mode_args}` slot per round, so the table and the
/// slot need each other: a slot nothing fills would reach the shell verbatim,
/// and a table with no slot would change nothing. The approved plan is
/// implemented by resuming the same session, so the table also needs
/// `[conversation]`.
pub(super) fn validate(descriptor: &HarnessDescriptor) -> Result<(), String> {
    let exec_has_slot = descriptor
        .dispatch
        .exec_template
        .as_deref()
        .is_some_and(|template| template.contains(SLOT));
    let resume_has_slot = descriptor
        .conversation
        .as_ref()
        .is_some_and(|conversation| conversation.resume_exec_template.contains(SLOT));
    if descriptor.plan_mode.is_none() {
        if exec_has_slot {
            return Err(format!(
                "dispatch.exec_template references {SLOT} but no [plan_mode] table fills it"
            ));
        }
        if resume_has_slot {
            return Err(format!(
                "conversation.resume_exec_template references {SLOT} but no [plan_mode] table \
                 fills it"
            ));
        }
        return Ok(());
    }
    if descriptor.conversation.is_none() {
        return Err(
            "[plan_mode] requires [conversation]: the approved plan is implemented by resuming \
             the same session"
                .into(),
        );
    }
    if !exec_has_slot {
        return Err(format!(
            "[plan_mode] requires dispatch.exec_template to carry the {SLOT} slot its \
             plan_args/act_args fill"
        ));
    }
    if !resume_has_slot {
        return Err(format!(
            "[plan_mode] requires conversation.resume_exec_template to carry the {SLOT} slot its \
             plan_args/act_args fill"
        ));
    }
    Ok(())
}
