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
    ConversationStopReason, PlanSignal, ResponderOutcome, ResponderPolicy, TurnOrigin,
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
/// tool targeted it. The last such write wins, since an agent may revise its
/// plan within a round; its `content_field` is the plan, falling back to the
/// round's final message when the write carried none.
pub(super) fn plan_file_written(
    summary: &TranscriptSummary,
    plan_file: &PlanFileSection,
    home: &Path,
    eval_root: &Path,
) -> Option<PresentedPlan> {
    let root = plan_file.expanded_root(home).to_string_lossy().into_owned();
    let write = summary.tool_invocations.iter().rev().find(|invocation| {
        is_write_tool(&invocation.name)
            && invocation
                .args
                .as_ref()
                .and_then(path_arg)
                .is_some_and(|path| is_under(path, &root, eval_root))
    })?;
    let text = write
        .args
        .as_ref()
        .and_then(|args| args.get(&plan_file.content_field))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| summary.final_text.clone())?;
    Some(PresentedPlan {
        text,
        signal: PlanSignal::PlanFile,
    })
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
        return PlanDecision::Stop {
            reason: ConversationStopReason::PlanNotPresented,
            responder: None,
        };
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
    use crate::core::{ConversationStopReason, PlanSignal, ToolInvocation};

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
    }

    #[test]
    fn without_a_plan_file_or_responder_the_phase_stops_plan_not_presented() {
        let consultation = Consultation {
            task_prompt: "Add caching.",
            prior_replies: &[],
            final_message: "Which file?",
            planning: true,
        };
        let PlanDecision::Stop { reason, responder } = decide(None, None, 1, &consultation, None)
        else {
            panic!("nothing can approve a plan nobody presented");
        };
        assert_eq!(reason, ConversationStopReason::PlanNotPresented);
        assert!(responder.is_none());
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
