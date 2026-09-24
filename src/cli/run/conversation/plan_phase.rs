//! The planning phase of a plan-mode session.
//!
//! The driver in [`super`] runs the rounds; this decides, after each round the
//! agent spent in the harness's plan mode, whether it has presented its plan
//! (approve it and switch to act mode), asked something (answer and stay in
//! plan mode), or neither (stop).

use std::path::Path;

use serde_json::Value;

use crate::adapters::TranscriptSummary;
use crate::adapters::descriptor::PlanFileSection;
use crate::core::{
    ConversationStopReason, PlanSignal, ResponderOutcome, ResponderPolicy, ToolInvocation,
    TurnOrigin,
};
use crate::sandbox::{is_under, is_write_tool, path_arg};

use super::responder::{Consultation, ResponderRuntime};
use super::turn_plan::{NextTurn, next_from_verdict};

/// What the runner says to approve a plan. Fixed, so the transition from
/// planning to implementation is identical in every run and both arms.
pub(super) const PLAN_APPROVAL_PROMPT: &str = "The plan is approved. Implement it now.";

/// A plan the agent has presented, and what marked it as presented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PresentedPlan {
    pub(super) text: String,
    pub(super) signal: PlanSignal,
}

/// What follows a plan-mode round.
pub(super) enum PlanDecision {
    /// Approve the plan and continue the session in act mode.
    Approve(PresentedPlan),
    /// Answer the agent and stay in plan mode.
    Answer { text: String, origin: TurnOrigin },
    /// Halt and record why. A normal outcome, not a failure.
    Stop {
        reason: ConversationStopReason,
        responder: Option<ResponderOutcome>,
    },
}

/// The plan file the round wrote, when the harness declares one and a write
/// tool targeted it. The last-touched plan file is the plan. An agent may
/// revise it within a round, often with in-place edits that carry no
/// `content_field`, so its content is rebuilt by replaying every write to that
/// path in order. When the replay cannot reproduce the file, the round's final
/// message stands in and the signal says so rather than claiming the file.
pub(super) fn plan_file_written(
    summary: &TranscriptSummary,
    plan_file: &PlanFileSection,
    home: &Path,
    eval_root: &Path,
) -> Option<PresentedPlan> {
    let root = plan_file.expanded_root(home).to_string_lossy().into_owned();
    let path = summary
        .tool_invocations
        .iter()
        .rev()
        .find_map(|invocation| plan_target(invocation, &root, eval_root))?;
    let writes: Vec<&ToolInvocation> = summary
        .tool_invocations
        .iter()
        .filter(|invocation| plan_target(invocation, &root, eval_root) == Some(path))
        .collect();
    match replay(&writes, &plan_file.content_field) {
        Some(text) => Some(PresentedPlan {
            text,
            signal: PlanSignal::PlanFile,
        }),
        None => summary.final_text.clone().map(|text| PresentedPlan {
            text,
            signal: PlanSignal::FinalMessage,
        }),
    }
}

/// The path a write-tool call targeted, when it lies under the plan root.
fn plan_target<'a>(
    invocation: &'a ToolInvocation,
    root: &str,
    eval_root: &Path,
) -> Option<&'a str> {
    if !is_write_tool(&invocation.name) {
        return None;
    }
    invocation
        .args
        .as_ref()
        .and_then(path_arg)
        .filter(|path| is_under(path, root, eval_root))
}

/// Rebuild a file from the writes that targeted it: a write carrying
/// `content_field` replaces the content, and an edit (`old_string` /
/// `new_string` / `replace_all`, or a list of them under `edits`) applies with
/// the harness's own matching rules. A write the harness rejected changed
/// nothing and is skipped. `None` when there is no base content or an edit the
/// harness accepted cannot be reproduced, so the rebuilt text would be a guess.
fn replay(writes: &[&ToolInvocation], content_field: &str) -> Option<String> {
    let mut content: Option<String> = None;
    for invocation in writes.iter().filter(|invocation| !rejected(invocation)) {
        let args = invocation.args.as_ref()?;
        if let Some(text) = args.get(content_field).and_then(Value::as_str) {
            content = Some(text.to_string());
            continue;
        }
        let current = content.as_mut()?;
        match args.get("edits").and_then(Value::as_array) {
            Some(edits) => {
                for edit in edits {
                    apply_edit(current, edit)?;
                }
            }
            None => apply_edit(current, args)?,
        }
    }
    content
}

/// Apply one string replacement in place. As in the harness, the target must
/// occur exactly once unless `replace_all` is set.
fn apply_edit(content: &mut String, edit: &Value) -> Option<()> {
    let old = edit.get("old_string").and_then(Value::as_str)?;
    let new = edit.get("new_string").and_then(Value::as_str)?;
    let replace_all = edit
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let occurrences = if old.is_empty() {
        0
    } else {
        content.matches(old).count()
    };
    if occurrences == 0 || (occurrences > 1 && !replace_all) {
        return None;
    }
    *content = if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    };
    Some(())
}

/// Whether the harness reported the tool call as failed, so it changed nothing.
fn rejected(invocation: &ToolInvocation) -> bool {
    invocation
        .result
        .as_ref()
        .and_then(Value::as_str)
        .is_some_and(|result| result.contains("<tool_use_error>"))
}

/// Decide what follows a plan-mode round. A presented plan is approved without
/// consulting anyone; otherwise the responder, when the eval has one, answers
/// the agent or judges the plan ready (`done`); with neither there is nothing
/// to approve and the run stops.
pub(super) fn decide(
    presented: Option<PresentedPlan>,
    responder: Option<(&ResponderPolicy, &ResponderRuntime<'_>)>,
    followup: u32,
    consultation: &Consultation<'_>,
    previous_reply: Option<&str>,
) -> PlanDecision {
    if let Some(presented) = presented {
        return PlanDecision::Approve(presented);
    }
    let Some((policy, runtime)) = responder else {
        // Nothing else can say the plan is ready, so the round's closing
        // message is it. The dispatch prompt asked the agent for exactly that,
        // which is what makes this a signal rather than a guess.
        return PlanDecision::Approve(PresentedPlan {
            text: consultation.final_message.to_string(),
            signal: PlanSignal::FinalMessage,
        });
    };
    let verdict = runtime.consult(followup, consultation, previous_reply);
    match next_from_verdict(policy, consultation.prior_replies.len() as u32, verdict) {
        // A planning `done` approves the plan rather than ending the
        // conversation, so its outcome is deliberately not carried onto the
        // record: `plan.signal` names the responder as the approver, and the
        // consultation itself stays on disk under `responder/turn-<n>/`.
        NextTurn::Done { .. } => PlanDecision::Approve(PresentedPlan {
            text: consultation.final_message.to_string(),
            signal: PlanSignal::Responder,
        }),
        NextTurn::Stop { reason, responder } => PlanDecision::Stop { reason, responder },
        NextTurn::Deliver { text, origin } => PlanDecision::Answer {
            text,
            origin: origin.expect("a responder-derived turn names its origin"),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::adapters::TranscriptSummary;
    use crate::adapters::descriptor::PlanFileSection;
    use crate::core::{PlanSignal, ToolInvocation};

    fn plan_file() -> PlanFileSection {
        PlanFileSection {
            root: "~/.claude/plans".into(),
            content_field: "content".into(),
        }
    }

    fn write(path: &str, content: Option<&str>, ordinal: u32) -> ToolInvocation {
        let mut args = json!({ "file_path": path });
        if let Some(content) = content {
            args["content"] = json!(content);
        }
        ToolInvocation {
            name: "Write".into(),
            args: Some(args),
            ordinal,
            result: None,
        }
    }

    fn edit(path: &str, old: &str, new: &str, replace_all: bool, ordinal: u32) -> ToolInvocation {
        ToolInvocation {
            name: "Edit".into(),
            args: Some(json!({
                "file_path": path,
                "old_string": old,
                "new_string": new,
                "replace_all": replace_all,
            })),
            ordinal,
            result: None,
        }
    }

    fn multi_edit(path: &str, edits: &[(&str, &str)], ordinal: u32) -> ToolInvocation {
        let edits: Vec<Value> = edits
            .iter()
            .map(|(old, new)| json!({ "old_string": old, "new_string": new }))
            .collect();
        ToolInvocation {
            name: "MultiEdit".into(),
            args: Some(json!({ "file_path": path, "edits": edits })),
            ordinal,
            result: None,
        }
    }

    fn rejected(mut invocation: ToolInvocation) -> ToolInvocation {
        invocation.result = Some(json!(
            "<tool_use_error>String to replace not found in file.</tool_use_error>"
        ));
        invocation
    }

    const PLAN: &str = "/Users/someone/.claude/plans/fix.md";

    fn presented(tool_invocations: Vec<ToolInvocation>) -> PresentedPlan {
        let summary = summary(tool_invocations, "Summary of the plan.");
        plan_file_written(
            &summary,
            &plan_file(),
            Path::new("/Users/someone"),
            Path::new("/env"),
        )
        .expect("a plan file write presents a plan")
    }

    #[test]
    fn an_edit_after_the_write_is_replayed_onto_the_plan() {
        let plan = presented(vec![
            write(PLAN, Some("1. Update src/a.ts\n2. Test it\n"), 0),
            edit(PLAN, "src/a.ts", "src/app/a.ts", false, 1),
        ]);
        assert_eq!(plan.text, "1. Update src/app/a.ts\n2. Test it\n");
        assert_eq!(plan.signal, PlanSignal::PlanFile);
    }

    #[test]
    fn a_replace_all_edit_replaces_every_occurrence() {
        let plan = presented(vec![
            write(PLAN, Some("toggle-favourite then toggle-favourite\n"), 0),
            edit(PLAN, "favourite", "favorite", true, 1),
        ]);
        assert_eq!(plan.text, "toggle-favorite then toggle-favorite\n");
        assert_eq!(plan.signal, PlanSignal::PlanFile);
    }

    #[test]
    fn a_multi_edit_applies_its_edits_in_sequence() {
        let plan = presented(vec![
            write(PLAN, Some("1. TBD\n2. Audit rounding\n"), 0),
            multi_edit(
                PLAN,
                &[("TBD", "Add the badge"), ("2. Audit rounding\n", "")],
                1,
            ),
        ]);
        assert_eq!(plan.text, "1. Add the badge\n");
        assert_eq!(plan.signal, PlanSignal::PlanFile);
    }

    #[test]
    fn an_edit_the_harness_rejected_leaves_the_plan_unchanged() {
        let plan = presented(vec![
            write(PLAN, Some("1. Fix it\n"), 0),
            rejected(edit(PLAN, "missing", "never applied", false, 1)),
            edit(PLAN, "Fix it", "Fix it properly", false, 2),
        ]);
        assert_eq!(plan.text, "1. Fix it properly\n");
        assert_eq!(plan.signal, PlanSignal::PlanFile);
    }

    #[test]
    fn an_edit_that_cannot_be_replayed_falls_back_to_the_final_message() {
        let plan = presented(vec![
            write(PLAN, Some("1. Fix it\n"), 0),
            edit(PLAN, "text the plan never contained", "new", false, 1),
        ]);
        assert_eq!(plan.text, "Summary of the plan.");
        assert_eq!(plan.signal, PlanSignal::FinalMessage);
    }

    #[test]
    fn an_ambiguous_single_edit_cannot_be_replayed() {
        let plan = presented(vec![
            write(PLAN, Some("a and a\n"), 0),
            edit(PLAN, "a", "b", false, 1),
        ]);
        assert_eq!(plan.text, "Summary of the plan.");
        assert_eq!(plan.signal, PlanSignal::FinalMessage);
    }

    #[test]
    fn an_edit_without_an_earlier_write_falls_back_to_the_final_message() {
        let plan = presented(vec![edit(PLAN, "old", "new", false, 0)]);
        assert_eq!(plan.text, "Summary of the plan.");
        assert_eq!(plan.signal, PlanSignal::FinalMessage);
    }

    #[test]
    fn only_the_last_touched_plan_file_is_replayed() {
        let other = "/Users/someone/.claude/plans/other.md";
        let plan = presented(vec![
            write(PLAN, Some("1. First plan\n"), 0),
            write(other, Some("1. Second plan\n"), 1),
            edit(PLAN, "First", "Revised first", false, 2),
            edit(other, "Second", "Revised second", false, 3),
        ]);
        assert_eq!(plan.text, "1. Revised second plan\n");
        assert_eq!(plan.signal, PlanSignal::PlanFile);
    }

    fn summary(tool_invocations: Vec<ToolInvocation>, final_text: &str) -> TranscriptSummary {
        TranscriptSummary {
            tool_invocations,
            events: Vec::new(),
            session_id: Some("s".into()),
            total_tokens: None,
            duration_ms: None,
            final_text: Some(final_text.into()),
        }
    }

    #[test]
    fn a_root_with_a_leading_tilde_expands_to_the_home_directory() {
        let home = Path::new("/Users/someone");
        assert_eq!(
            plan_file().expanded_root(home),
            Path::new("/Users/someone/.claude/plans")
        );
        let absolute = PlanFileSection {
            root: "/var/plans".into(),
            content_field: "content".into(),
        };
        assert_eq!(absolute.expanded_root(home), Path::new("/var/plans"));
    }

    #[test]
    fn a_plan_file_write_this_round_presents_the_plan_with_its_content() {
        let home = Path::new("/Users/someone");
        let summary = summary(
            vec![write(
                "/Users/someone/.claude/plans/fix.md",
                Some("1. Fix it\n"),
                0,
            )],
            "Here is the plan.",
        );
        let presented = plan_file_written(&summary, &plan_file(), home, Path::new("/env"))
            .expect("the plan file write is the signal");
        assert_eq!(presented.text, "1. Fix it\n");
        assert_eq!(presented.signal, PlanSignal::PlanFile);
    }

    #[test]
    fn the_last_plan_file_write_wins() {
        let home = Path::new("/Users/someone");
        let summary = summary(
            vec![
                write("/Users/someone/.claude/plans/fix.md", Some("draft"), 0),
                write("/Users/someone/.claude/plans/fix.md", Some("final"), 1),
            ],
            "Done planning.",
        );
        let presented = plan_file_written(&summary, &plan_file(), home, Path::new("/env")).unwrap();
        assert_eq!(presented.text, "final");
    }

    #[test]
    fn a_write_elsewhere_is_not_a_plan() {
        let home = Path::new("/Users/someone");
        let summary = summary(
            vec![write("/env/notes.md", Some("not a plan"), 0)],
            "Which file?",
        );
        assert!(plan_file_written(&summary, &plan_file(), home, Path::new("/env")).is_none());
    }

    #[test]
    fn a_plan_file_write_without_content_falls_back_to_the_final_text() {
        let home = Path::new("/Users/someone");
        let summary = summary(
            vec![write("/Users/someone/.claude/plans/fix.md", None, 0)],
            "The plan: fix it.",
        );
        let presented = plan_file_written(&summary, &plan_file(), home, Path::new("/env")).unwrap();
        assert_eq!(presented.text, "The plan: fix it.");
        assert_eq!(presented.signal, PlanSignal::FinalMessage);
    }

    /// The last rung of the ladder. With no plan file to read and no responder
    /// to ask, the planning round's final message *is* the plan — the dispatch
    /// prompt told the agent to end the turn with it — so the run gets an
    /// inspectable `plan.md` instead of stopping empty-handed.
    #[test]
    fn without_a_plan_file_or_responder_the_final_message_is_the_plan() {
        let consultation = Consultation {
            task_prompt: "Add caching.",
            prior_replies: &[],
            final_message: "1. Add an LRU.\n2. Test it.\n",
            planning: true,
        };
        let PlanDecision::Approve(approved) = decide(None, None, 1, &consultation, None) else {
            panic!("the final message stands in for a plan file the harness never writes");
        };
        assert_eq!(approved.text, "1. Add an LRU.\n2. Test it.\n");
        assert_eq!(approved.signal, PlanSignal::FinalMessage);
    }

    #[test]
    fn a_presented_plan_is_approved_without_consulting_anyone() {
        let consultation = Consultation {
            task_prompt: "Add caching.",
            prior_replies: &[],
            final_message: "Here is the plan.",
            planning: true,
        };
        let presented = PresentedPlan {
            text: "1. Fix it\n".into(),
            signal: PlanSignal::PlanFile,
        };
        let PlanDecision::Approve(approved) = decide(Some(presented), None, 1, &consultation, None)
        else {
            panic!("a presented plan is approved");
        };
        assert_eq!(approved.text, "1. Fix it\n");
        assert_eq!(approved.signal, PlanSignal::PlanFile);
    }
}
