//! What the user says next, and why.
//!
//! The driver in [`super`] owns running a round and recording it; this owns the
//! decision between rounds. Both shapes of multi-turn eval resolve to one
//! [`TurnPlan`], so the driver has a single delivery path whatever it is running.

use anyhow::Context;
use regex::Regex;

use crate::cli::run::dispatch::DispatchTask;
use crate::core::{ConversationStopReason, DeliverWhen, ResponderPolicy, ScriptedTurn, TurnOrigin};

use super::{TurnSource, responder};

/// Where a task's follow-up turns come from. Resolving this once, up front,
/// keeps the driver's loop to a single delivery path whatever shape of task it
/// is running.
pub(super) enum TurnPlan<'a> {
    /// A one-shot task: dispatched once, with nothing to follow up.
    OneShot,
    /// An authored script, delivered in order behind its gates.
    Scripted(&'a [ScriptedTurn]),
    /// A policy that derives each turn from what the agent just said.
    Responder(&'a ResponderPolicy),
}

/// What the plan wants to happen after a round.
pub(super) enum NextTurn {
    /// The conversation is finished — a script ran out, or the agent stopped
    /// asking.
    Done,
    /// Halt and record why. A normal outcome, not a failure.
    Stop(ConversationStopReason),
    /// Send this as the next user turn. `origin` names the responder rule that
    /// produced it, and is absent for an authored scripted turn.
    Deliver {
        text: String,
        origin: Option<TurnOrigin>,
    },
}

impl<'a> TurnPlan<'a> {
    pub(super) fn for_task(task: &'a DispatchTask) -> Self {
        // `turns` and `responder` are mutually exclusive by config validation,
        // so the order here only decides which wins if that gate is ever
        // bypassed — the authored script does, being the more explicit of the two.
        match (task.turns.as_deref(), task.responder.as_ref()) {
            (Some(turns), _) if !turns.is_empty() => Self::Scripted(turns),
            (_, Some(responder)) => Self::Responder(responder),
            _ => Self::OneShot,
        }
    }

    /// Whether this plan can resume a session, and therefore needs the
    /// harness's resume template.
    pub(super) fn delivers_followups(&self) -> bool {
        !matches!(self, Self::OneShot)
    }

    pub(super) fn source(&self) -> TurnSource {
        match self {
            Self::Responder(_) => TurnSource::Responder,
            // A one-shot task delivers nothing, so its source never reaches the
            // wording — either arm would do, and the scripted one is the older.
            Self::OneShot | Self::Scripted(_) => TurnSource::Scripted,
        }
    }

    /// Decide what follows a round.
    ///
    /// `preceding_assistant` is every assistant message of the round joined,
    /// which is what a scripted gate has always been evaluated against.
    /// `final_message` is just the round's last message — the one a user would
    /// actually be answering — and is what the responder reads.
    pub(super) fn next_turn(
        &self,
        delivered: u32,
        preceding_assistant: &str,
        final_message: &str,
    ) -> anyhow::Result<NextTurn> {
        match self {
            Self::OneShot => Ok(NextTurn::Done),
            Self::Scripted(turns) => {
                let Some(turn) = turns.get(delivered as usize) else {
                    return Ok(NextTurn::Done);
                };
                if let Some(reason) = unmet_gate(turn, preceding_assistant)? {
                    return Ok(NextTurn::Stop(reason));
                }
                Ok(NextTurn::Deliver {
                    text: turn.prompt.clone(),
                    origin: None,
                })
            }
            Self::Responder(policy) => Ok(responder_turn(policy, delivered, final_message)),
        }
    }
}

/// Classify the agent's last message, then bound the result.
///
/// Classification comes first deliberately: an agent that has stopped asking
/// has finished the task, and finishing on the last permitted turn is a
/// completion, not a run that ran out of budget.
fn responder_turn(policy: &ResponderPolicy, delivered: u32, final_message: &str) -> NextTurn {
    match responder::read(final_message) {
        responder::Reading::NoQuestion => NextTurn::Done,
        responder::Reading::Unanswerable => {
            NextTurn::Stop(ConversationStopReason::ResponderCannotAnswer)
        }
        responder::Reading::Answer { text, origin } => {
            if delivered >= policy.max_turns() {
                return NextTurn::Stop(ConversationStopReason::MaxTurnsReached);
            }
            NextTurn::Deliver {
                text,
                origin: Some(origin),
            }
        }
    }
}

fn unmet_gate(
    turn: &ScriptedTurn,
    preceding_assistant: &str,
) -> anyhow::Result<Option<ConversationStopReason>> {
    if turn.deliver_when == DeliverWhen::Always {
        return Ok(None);
    }
    if !preceding_assistant.contains('?') {
        return Ok(Some(ConversationStopReason::AgentDidNotAsk));
    }
    if let Some(pattern) = &turn.agent_response_matches {
        let regex = Regex::new(pattern)
            .with_context(|| format!("invalid agent_response_matches regex {pattern:?}"))?;
        if !regex.is_match(preceding_assistant) {
            return Ok(Some(ConversationStopReason::AgentResponseMismatch));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::unmet_gate;
    use crate::core::{ConversationStopReason, DeliverWhen, ScriptedTurn};

    fn conditional(pattern: Option<&str>) -> ScriptedTurn {
        ScriptedTurn {
            prompt: "follow up".into(),
            deliver_when: DeliverWhen::AgentAsks,
            agent_response_matches: pattern.map(str::to_string),
        }
    }

    #[test]
    fn agent_asks_requires_a_question_mark() {
        assert_eq!(
            unmet_gate(&conditional(None), "Please provide the timezone.").unwrap(),
            Some(ConversationStopReason::AgentDidNotAsk)
        );
        assert_eq!(
            unmet_gate(&conditional(None), "Which timezone?").unwrap(),
            None
        );
    }

    #[test]
    fn response_pattern_is_an_additional_compatibility_gate() {
        assert_eq!(
            unmet_gate(&conditional(Some("(?i)time ?zone")), "Which locale?").unwrap(),
            Some(ConversationStopReason::AgentResponseMismatch)
        );
        assert_eq!(
            unmet_gate(
                &conditional(Some("(?i)time ?zone")),
                "Which timezone should I use?"
            )
            .unwrap(),
            None
        );
    }
}
