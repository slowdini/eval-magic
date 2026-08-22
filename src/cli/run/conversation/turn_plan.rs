//! What the user says next, and why.
//!
//! The driver in [`super`] owns running a round and recording it; this owns the
//! decision between rounds. Both shapes of multi-turn eval resolve to one
//! [`TurnPlan`], so the driver has a single delivery path whatever it is running.

use anyhow::Context;
use regex::Regex;

use crate::cli::run::dispatch::DispatchTask;
use crate::core::{
    ConversationStopReason, DeliverWhen, ResponderEnding, ResponderKind, ResponderOutcome,
    ResponderPolicy, ResponderStopCause, ScriptedTurn, TurnOrigin,
};

use super::{TurnSource, responder};
use responder::{Consultation, ResponderRuntime, Verdict};

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
    /// The conversation is finished — a script ran out, or the responder
    /// judged the agent done. `responder` records that judgement when it was
    /// the responder that made it.
    Done { responder: Option<ResponderOutcome> },
    /// Halt and record why. A normal outcome, not a failure.
    Stop {
        reason: ConversationStopReason,
        responder: Option<ResponderOutcome>,
    },
    /// Send this as the next user turn. `origin` names the responder that
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
        consultation: &Consultation<'_>,
        runtime: Option<&ResponderRuntime<'_>>,
        previous_reply: Option<&str>,
    ) -> anyhow::Result<NextTurn> {
        match self {
            Self::OneShot => Ok(NextTurn::Done { responder: None }),
            Self::Scripted(turns) => {
                let Some(turn) = turns.get(delivered as usize) else {
                    return Ok(NextTurn::Done { responder: None });
                };
                if let Some(reason) = unmet_gate(turn, preceding_assistant)? {
                    return Ok(NextTurn::Stop {
                        reason,
                        responder: None,
                    });
                }
                Ok(NextTurn::Deliver {
                    text: turn.prompt.clone(),
                    origin: None,
                })
            }
            Self::Responder(policy) => {
                let runtime = runtime
                    .expect("a responder plan resolved its runtime alongside the plan itself");
                let verdict =
                    runtime.consult(delivered.saturating_add(1), consultation, previous_reply);
                Ok(next_from_verdict(policy, delivered, verdict))
            }
        }
    }
}

/// Turn one consultation into the next step, then bound the result.
///
/// Classification comes first deliberately: an agent that has stopped asking
/// has finished the task, and finishing on the last permitted turn is a
/// completion, not a run that ran out of budget.
fn next_from_verdict(
    policy: &ResponderPolicy,
    delivered: u32,
    verdict: Result<Verdict, ResponderStopCause>,
) -> NextTurn {
    let verdict = match verdict {
        Ok(verdict) => verdict,
        Err(cause) => return cannot_answer(cause, None),
    };
    match verdict {
        Verdict::Done { rationale } => NextTurn::Done {
            responder: Some(ResponderOutcome {
                ending: ResponderEnding::Done,
                cause: None,
                rationale,
            }),
        },
        Verdict::CannotAnswer { rationale } => {
            cannot_answer(ResponderStopCause::Declined, rationale)
        }
        Verdict::Answer { reply, rationale } => {
            if delivered >= policy.max_turns() {
                // The bound is the runner's decision, not a verdict, so no
                // responder outcome is recorded against it.
                return NextTurn::Stop {
                    reason: ConversationStopReason::MaxTurnsReached,
                    responder: None,
                };
            }
            NextTurn::Deliver {
                text: reply,
                origin: Some(TurnOrigin {
                    responder: ResponderKind::Llm,
                    rationale,
                }),
            }
        }
    }
}

/// Every way the responder fails to produce a usable reply ends the run the
/// same way; the cause is what tells an honest refusal from a broken dispatch.
fn cannot_answer(cause: ResponderStopCause, rationale: Option<String>) -> NextTurn {
    NextTurn::Stop {
        reason: ConversationStopReason::ResponderCannotAnswer,
        responder: Some(ResponderOutcome {
            ending: ResponderEnding::CannotAnswer,
            cause: Some(cause),
            rationale,
        }),
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
    use super::{NextTurn, next_from_verdict, unmet_gate};
    use crate::cli::run::conversation::responder::Verdict;
    use crate::core::{
        ConversationStopReason, DeliverWhen, ResponderEnding, ResponderKind, ResponderPolicy,
        ResponderStopCause, ScriptedTurn,
    };

    fn policy(max_turns: u32) -> ResponderPolicy {
        ResponderPolicy {
            kind: ResponderKind::Llm,
            max_turns: Some(max_turns),
        }
    }

    fn answer(reply: &str) -> Verdict {
        Verdict::Answer {
            reply: reply.to_string(),
            rationale: Some("the simplest option".to_string()),
        }
    }

    #[test]
    fn an_answer_below_the_bound_is_delivered_with_its_origin() {
        let NextTurn::Deliver { text, origin } =
            next_from_verdict(&policy(8), 0, Ok(answer("Use the LRU.")))
        else {
            panic!("an answer under the bound is delivered");
        };
        assert_eq!(text, "Use the LRU.");
        let origin = origin.expect("a derived turn names its origin");
        assert_eq!(origin.responder, ResponderKind::Llm);
        assert_eq!(origin.rationale.as_deref(), Some("the simplest option"));
    }

    /// Classification comes before the bound: an agent that finishes on its
    /// last permitted turn completed, and only one still asking has run out.
    #[test]
    fn an_answer_at_the_bound_stops_without_delivering() {
        let NextTurn::Stop { reason, responder } =
            next_from_verdict(&policy(2), 2, Ok(answer("Use the LRU.")))
        else {
            panic!("the bound stops the conversation");
        };
        assert_eq!(reason, ConversationStopReason::MaxTurnsReached);
        assert!(
            responder.is_none(),
            "the bound is the runner's decision, not the responder's verdict"
        );
    }

    #[test]
    fn a_done_verdict_ends_the_conversation_and_records_why() {
        let NextTurn::Done { responder } = next_from_verdict(
            &policy(8),
            1,
            Ok(Verdict::Done {
                rationale: Some("the agent reported the cache in place".to_string()),
            }),
        ) else {
            panic!("done ends the conversation");
        };
        let outcome = responder.expect("a responder-ended conversation records how");
        assert_eq!(outcome.ending, ResponderEnding::Done);
        assert_eq!(outcome.cause, None);
        assert_eq!(
            outcome.rationale.as_deref(),
            Some("the agent reported the cache in place")
        );
    }

    #[test]
    fn a_declined_verdict_stops_with_the_declined_cause() {
        let NextTurn::Stop { reason, responder } = next_from_verdict(
            &policy(8),
            0,
            Ok(Verdict::CannotAnswer {
                rationale: Some("it asked for a credential I was never given".to_string()),
            }),
        ) else {
            panic!("cannot_answer stops the conversation");
        };
        assert_eq!(reason, ConversationStopReason::ResponderCannotAnswer);
        let outcome = responder.expect("the stop records why");
        assert_eq!(outcome.ending, ResponderEnding::CannotAnswer);
        assert_eq!(outcome.cause, Some(ResponderStopCause::Declined));
    }

    /// A broken dispatch and an honest refusal end the run the same way — the
    /// task is unfinished either way — but the cause tells them apart.
    #[test]
    fn a_failed_consultation_stops_with_its_own_cause() {
        let NextTurn::Stop { reason, responder } =
            next_from_verdict(&policy(8), 0, Err(ResponderStopCause::DispatchTimedOut))
        else {
            panic!("a failed consultation stops the conversation");
        };
        assert_eq!(reason, ConversationStopReason::ResponderCannotAnswer);
        let outcome = responder.expect("the stop records why");
        assert_eq!(outcome.ending, ResponderEnding::CannotAnswer);
        assert_eq!(outcome.cause, Some(ResponderStopCause::DispatchTimedOut));
        assert_eq!(outcome.rationale, None);
    }

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
