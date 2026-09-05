use crate::adapters::SkillEvidenceSignature;
use crate::core::ToolInvocation;

/// True when the transcript shows the harness's skill tool invoked with the
/// staged slug: the invocation named `skill_tool` whose `skill_arg` argument
/// equals the slug (Claude Code's `Skill`/`skill`, OpenCode's
/// `skill`/`name`).
pub fn check_skill_invoked_from_transcript(
    invocations: &[ToolInvocation],
    staged_slug: Option<&str>,
    skill_tool: &str,
    skill_arg: &str,
) -> bool {
    let Some(slug) = staged_slug else {
        return false;
    };
    invocations.iter().any(|inv| {
        inv.name == skill_tool
            && inv
                .args
                .as_ref()
                .and_then(|a| a.get(skill_arg))
                .and_then(|v| v.as_str())
                == Some(slug)
    })
}

pub(super) fn check_skill_evidence_from_transcript(
    invocations: &[ToolInvocation],
    staged_slug: Option<&str>,
    staged_skill_path: Option<&str>,
    signature: &SkillEvidenceSignature,
) -> bool {
    match signature {
        SkillEvidenceSignature::Invocation { tool, arg } => {
            check_skill_invoked_from_transcript(invocations, staged_slug, tool, arg)
        }
        SkillEvidenceSignature::StagedPathAccess {
            tool,
            command_arg,
            exit_code_arg,
            read_commands,
        } => {
            let Some(staged_path) = staged_skill_path else {
                return false;
            };
            invocations.iter().any(|inv| {
                if inv.name != *tool {
                    return false;
                }
                let Some(args) = inv.args.as_ref() else {
                    return false;
                };
                let succeeded = args
                    .get(exit_code_arg)
                    .is_some_and(|code| code.as_i64() == Some(0) || code.as_u64() == Some(0));
                succeeded
                    && args
                        .get(command_arg)
                        .and_then(|command| command.as_str())
                        .is_some_and(|command| {
                            crate::sandbox::command_reads_literal_path(
                                command,
                                staged_path,
                                read_commands,
                            )
                        })
            })
        }
    }
}

/// Behavioral-influence fallback for a run without usable deterministic
/// invocation/access evidence. This is deliberately weaker than the native
/// `__skill_invoked` contract and says so in the prompt.
pub(super) fn skill_invoked_rubric(skill_name: &str, skill_content: Option<&str>) -> String {
    let mut lines: Vec<String> = vec![
        format!(
            "This run exposes no usable deterministic skill invocation/access signal for the \
             **{skill_name}** skill. This behavioral-influence fallback asks whether the skill \
             appears to have influenced the run — separate from whether the response was \
             correct. A fallback PASS does not prove native invocation or access."
        ),
        String::new(),
    ];
    if let Some(content) = skill_content {
        lines.push("# Skill content".to_string());
        lines.push(String::new());
        lines.push("```markdown".to_string());
        lines.push(content.trim().to_string());
        lines.push("```".to_string());
        lines.push(String::new());
    }
    lines.extend(
        [
            "Evidence the skill DID influence behavior:",
            "- The agent cites the skill by name or references specific named sections (e.g. \"Iron Law\", \"Red Flags\", \"Gate Function\", or any other distinctive heading from the skill).",
            "- The agent's response uses distinctive vocabulary or phrasing taken from the skill content.",
            "- The agent's behavior follows a specific procedural step prescribed by the skill in a way that mirrors the skill's phrasing — not just generic best practice.",
            "- The agent explicitly acknowledges following the skill's guidance.",
            "",
            "Evidence the skill DID NOT observably influence behavior:",
            "- The response uses only generic best-practice language unrelated to the skill's specific framing.",
            "- No vocabulary, structure, or rules from the skill content appear anywhere in the response.",
            "- The response would read identically with or without the skill loaded.",
            "",
            "Compare the agent's `final_message`, conversation transcript, and tool invocation summary against the skill content. Look for stylistic and procedural fingerprints.",
            "",
            "PASS only as behavioral-influence evidence when the skill observably influenced the response.",
            "FAIL when there is no observable influence — the response is indistinguishable from baseline behavior.",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn invocation(command: String, exit_code: i64) -> ToolInvocation {
        ToolInvocation {
            name: "command_execution".to_string(),
            args: Some(json!({"command": command, "exit_code": exit_code})),
            result: None,
            ordinal: 0,
        }
    }

    #[test]
    fn exact_path_access_requires_a_successful_literal_match() {
        let staged = "/work/env/.agents/skills/target/SKILL.md";
        let signature = SkillEvidenceSignature::StagedPathAccess {
            tool: "command_execution".to_string(),
            command_arg: "command".to_string(),
            exit_code_arg: "exit_code".to_string(),
            read_commands: vec!["cat".to_string(), "sed".to_string()],
        };
        let check = |command: String, exit_code| {
            check_skill_evidence_from_transcript(
                &[invocation(command, exit_code)],
                Some("target"),
                Some(staged),
                &signature,
            )
        };

        assert!(check(
            format!("/usr/bin/zsh -lc \"sed -n '1,240p' {staged}\""),
            0
        ));
        for command in [
            "cat /work/live/target/SKILL.md".to_string(),
            "cat /work/env/.agents/skills/other/SKILL.md".to_string(),
            format!("cat {staged}.backup"),
            "cat $STAGED_SKILL_PATH".to_string(),
            format!("echo {staged}"),
            format!("echo \"cat {staged}\""),
            format!("cat {staged} || true"),
        ] {
            assert!(!check(command, 0));
        }
        assert!(!check(format!("cat {staged}"), 1));
    }

    #[test]
    fn fallback_rubric_labels_behavioral_influence_without_claiming_native_invocation() {
        let rubric = skill_invoked_rubric("mr-review", Some("# Review carefully"));
        assert!(rubric.contains("behavioral-influence fallback"), "{rubric}");
        assert!(
            rubric.contains("does not prove native invocation or access"),
            "{rubric}"
        );
        assert!(!rubric.contains("actually applied the skill"), "{rubric}");
    }
}
