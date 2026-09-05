use super::HarnessDescriptor;

/// Transcript ingest without a write/shell vocabulary makes the stray-writes
/// audit a silent no-op.
pub(super) fn check_tool_vocabulary(d: &HarnessDescriptor) -> Result<(), String> {
    if d.transcript.is_some() && (d.tools.write.is_empty() || d.tools.shell.is_empty()) {
        return Err(
            "[transcript] is declared but [tools] write/shell are empty; \
             detect-stray-writes would audit nothing — declare the harness's tool names"
                .into(),
        );
    }
    Ok(())
}

/// Exact-path access is an alternative to a native skill-tool signature. Its
/// tool must be declared as shell so the descriptor cannot assign command-line
/// semantics to an unrelated transcript item.
pub(super) fn check_skill_evidence(d: &HarnessDescriptor) -> Result<(), String> {
    let Some(transcript) = &d.transcript else {
        return Ok(());
    };
    let Some(access) = &transcript.skill_access else {
        return Ok(());
    };
    if !transcript.surfaces_skill_invocation {
        return Err(
            "transcript.skill_access requires surfaces_skill_invocation = true; otherwise the \
             descriptor declares deterministic evidence and disables it at the same time"
                .into(),
        );
    }
    if transcript.skill_tool.is_some() || transcript.skill_arg.is_some() {
        return Err(
            "transcript.skill_access cannot be combined with skill_tool or skill_arg; declare \
             either exact-path access or a native skill-tool signature"
                .into(),
        );
    }
    if !d.tools.shell.contains(&access.tool) {
        return Err(format!(
            "transcript.skill_access.tool {:?} is not declared in tools.shell; exact-path \
             command evidence must use a shell tool",
            access.tool
        ));
    }
    if access.command_arg == access.exit_code_arg {
        return Err(
            "transcript.skill_access command_arg and exit_code_arg must name distinct fields"
                .into(),
        );
    }
    if access.read_commands.iter().any(|command| {
        command.is_empty()
            || command == "."
            || command == ".."
            || !command
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '+' | '.'))
    }) {
        return Err(
            "transcript.skill_access.read_commands must contain literal executable basenames; \
             paths, whitespace, and shell syntax are not allowed"
                .into(),
        );
    }
    Ok(())
}

/// Transcript ingest has one primary summary reader: a named code parser or
/// declarative summary outputs. Independent denial and session-surface readers
/// may compose around it.
pub(super) fn check_tiers(d: &HarnessDescriptor) -> Result<(), String> {
    let Some(transcript) = &d.transcript else {
        return Ok(());
    };
    match (&transcript.parser, &transcript.extract) {
        (Some(_), Some(extract)) if extract.has_summary_outputs() => {
            return Err(
                "[transcript] declares both a parser and primary extract outputs; ingest reads \
                 its summary through exactly one — drop one of them. A parser may coexist only \
                 with transcript.extract.session_surface. Layered overrides merge fields and \
                 cannot delete an inherited parser, so moving a harness from parser to extract \
                 needs a new label rather than an overlay"
                    .into(),
            );
        }
        (None, None) => {
            return Err(
                "[transcript] declares neither a parser nor an extract block; declare \
                 exactly one — name a parser capability or add [transcript.extract] — \
                 or drop the table and let llm_judge carry the grading"
                    .into(),
            );
        }
        _ => {}
    }
    if let Some(extract) = &transcript.extract {
        if extract.tools.is_none()
            && extract.final_text.is_none()
            && extract.assistant_messages.is_none()
            && extract.session_id.is_none()
            && extract.tokens.is_none()
            && extract.duration.is_none()
            && extract.session_surface.is_none()
        {
            return Err(
                "[transcript.extract] declares none of tools/final_text/assistant_messages/\
                 session_id/tokens/duration/session_surface; an empty extract block parses \
                 nothing — declare at least one output"
                    .into(),
            );
        }
        if let Some(duration) = &extract.duration
            && duration.field.is_some() == duration.timestamp_spread.is_some()
        {
            return Err(
                "[transcript.extract.duration] must declare exactly one of field \
                 (a millisecond pick) or timestamp_spread (last minus first timestamp)"
                    .into(),
            );
        }
        if let Some(surface) = &extract.session_surface
            && surface.skills_field.is_none()
            && surface.plugins_field.is_none()
        {
            return Err(
                "[transcript.extract.session_surface] must declare at least one of \
                 skills_field or plugins_field; without either roster field the transcript \
                 cannot report evidence"
                    .into(),
            );
        }
        if let Some(surface) = &extract.session_surface
            && surface.plugins_field.is_some()
            && surface.plugin_name_field.is_none()
        {
            return Err(
                "[transcript.extract.session_surface] plugins_field requires \
                 plugin_name_field so object entries can provide the namespace used to \
                 identify their skills"
                    .into(),
            );
        }
        if let Some(surface) = &extract.session_surface
            && surface.plugins_field.is_none()
            && (surface.plugin_name_field.is_some()
                || surface.plugin_id_field.is_some()
                || surface.plugin_version_field.is_some())
        {
            return Err(
                "[transcript.extract.session_surface] plugin_name_field, plugin_id_field, and \
                 plugin_version_field require plugins_field; without a plugin roster those \
                 mappings are unused"
                    .into(),
            );
        }
        if transcript.parser.is_none() && !extract.has_summary_outputs() {
            return Err(
                "[transcript.extract.session_surface] is auxiliary and requires a primary \
                 summary reader; name transcript.parser or declare summary outputs under \
                 [transcript.extract]"
                    .into(),
            );
        }
        if transcript.parser.is_none()
            && extract.final_text.is_none()
            && extract.assistant_messages.is_none()
        {
            return Err(
                "[transcript.extract] cannot recover the agent's final response; declare \
                 final_text or assistant_messages so record-runs has transcript-owned text to \
                 grade"
                    .into(),
            );
        }
    }
    Ok(())
}
