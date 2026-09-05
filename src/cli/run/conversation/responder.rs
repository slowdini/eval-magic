//! The responder: what the person who asked for the work says next.
//!
//! One small model, dispatched through the same harness as the agent under
//! test, consulted once after every round. It answers the agent's question,
//! judges the task finished, or says it cannot answer — and the runner never
//! delivers a reply that fails validation, because a reply nobody vouched for
//! silently changes what the agent was asked to do.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::adapters::descriptor_adapter::DescriptorAdapter;
use crate::adapters::harness::HarnessAdapter;
use crate::core::{ResponderStopCause, ShellOutcome, run_in_posix_shell};

use super::render_dispatch_command;

/// How long one consultation may run. It is capped separately from the task's
/// own budget so a hung responder stops the run as a responder failure rather
/// than eating the agent's remaining time and being recorded as an agent
/// timeout.
const CONSULT_TIMEOUT: Duration = Duration::from_secs(300);

/// The byte ceiling on a reply. A simulated user answers in sentences; well
/// past that means the responder started doing the agent's work, and putting
/// that in the transcript would credit the agent under test with work it did
/// not do.
const MAX_REPLY_BYTES: usize = 2_000;

/// The line the responder is told to write its verdict by. Named here because
/// the prompt states it and a test reads it back.
const VERDICT_PATH_LINE: &str = "Write your verdict as a JSON file to:";

/// What the responder is shown. Deliberately only what the agent already knows:
/// the request it was given, what the user has said since, and what it just
/// said. The eval's `expected_output` and assertions are the grading criteria,
/// and a responder that had read them could hand the agent the rubric.
pub(super) struct Consultation<'a> {
    pub(super) task_prompt: &'a str,
    pub(super) prior_replies: &'a [String],
    pub(super) final_message: &'a str,
    /// Whether the agent is still in the harness's plan mode. `done` then
    /// approves the plan instead of ending the task, and the prompt says so.
    pub(super) planning: bool,
}

/// What the responder decided. `Answer` carries a reply that has already passed
/// validation by the time it leaves [`ResponderRuntime::consult`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Verdict {
    Answer {
        reply: String,
        rationale: Option<String>,
    },
    Done {
        rationale: Option<String>,
    },
    CannotAnswer {
        rationale: Option<String>,
    },
}

/// The verdict file as written, before it is known to be usable. Every field
/// tolerates absence, following the judge's `JudgeResponse`: a sloppy responder
/// should fail one named validation gate, not blow up parsing.
#[derive(serde::Deserialize)]
struct RawVerdict {
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    reply: Option<String>,
    #[serde(default)]
    rationale: Option<String>,
}

/// Everything a consultation needs that does not change between rounds.
pub(super) struct ResponderRuntime<'a> {
    pub(super) adapter: &'a DescriptorAdapter,
    pub(super) model: Option<&'a str>,
    pub(super) agent_env: &'a BTreeMap<String, String>,
    /// Where consultations run and are captured — outside every task env, so
    /// nothing the responder does can reach the codebase under measurement or
    /// pick up its `CLAUDE.md` as instructions.
    pub(super) responder_dir: PathBuf,
    pub(super) deadline: Option<Instant>,
}

impl ResponderRuntime<'_> {
    /// Ask the responder what the user says after `round`. Every failure is a
    /// named cause rather than an error: the run stops mid-task, which is a
    /// recorded result, not a broken campaign.
    pub(super) fn consult(
        &self,
        round: u32,
        consultation: &Consultation<'_>,
        previous_reply: Option<&str>,
    ) -> Result<Verdict, ResponderStopCause> {
        let dir = self.responder_dir.join(format!("turn-{round}"));
        fs::create_dir_all(&dir).map_err(|_| ResponderStopCause::DispatchFailed)?;
        let prompt_path = dir.join("prompt.txt");
        let verdict_path = dir.join("verdict.json");

        // Clear any verdict left by an earlier dispatch of this task, or by a
        // consultation that was killed part way through writing one. The file's
        // presence is the only evidence that this consultation answered, so a
        // stale one would be read as a reply written about a different
        // conversation.
        match fs::remove_file(&verdict_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ResponderStopCause::DispatchFailed),
        }

        fs::write(&prompt_path, build_prompt(consultation, &verdict_path))
            .map_err(|_| ResponderStopCause::DispatchFailed)?;

        // Guard arguments are deliberately off, for the reason a judge's are:
        // this dispatch runs outside every guarded task env.
        let template = self
            .adapter
            .cli_exec_command(false, self.model, self.agent_env)
            .ok_or(ResponderStopCause::DispatchFailed)?;
        let command = render_dispatch_command(
            &template,
            &dir.to_string_lossy(),
            &prompt_path.to_string_lossy(),
            &dir,
        );

        let budget = match self.deadline {
            Some(deadline) => {
                CONSULT_TIMEOUT.min(deadline.saturating_duration_since(Instant::now()))
            }
            None => CONSULT_TIMEOUT,
        };
        match run_in_posix_shell(&command, &dir, self.agent_env, Some(budget)) {
            Ok(ShellOutcome::Exited(status)) if status.success() => {}
            Ok(ShellOutcome::Exited(_)) | Err(_) => return Err(ResponderStopCause::DispatchFailed),
            Ok(ShellOutcome::TimedOut) => return Err(ResponderStopCause::DispatchTimedOut),
        }

        let raw = fs::read_to_string(&verdict_path)
            .ok()
            .filter(|body| !body.trim().is_empty())
            .ok_or(ResponderStopCause::MissingVerdict)?;
        let verdict = parse_verdict(&raw)?;
        if let Verdict::Answer { reply, .. } = &verdict {
            validate_reply(reply, previous_reply)?;
        }
        Ok(verdict)
    }
}

/// Build one consultation's prompt. Pure, so what the responder is and is not
/// shown is testable without dispatching anything.
///
/// The agent's own message goes in verbatim, and an agent could in principle
/// write instructions to the responder into it. That is contained by the
/// runner reading the verdict from the path it chose rather than one parsed
/// out of anything: a redirected write is a missing verdict, which stops the
/// run.
fn build_prompt(consultation: &Consultation<'_>, verdict_path: &Path) -> String {
    let said_since = if consultation.prior_replies.is_empty() {
        String::new()
    } else {
        let replies: Vec<String> = consultation
            .prior_replies
            .iter()
            .enumerate()
            .map(|(index, reply)| format!("{}. {reply}", index + 1))
            .collect();
        format!("# What you have said since\n\n{}\n\n", replies.join("\n"))
    };

    // What `done` means depends on the phase: while the agent is planning it
    // approves the plan, afterwards it ends the task.
    let phase_note = if consultation.planning {
        "- The agent is in a planning phase: it may only read and plan, and must present a plan\n  \
         for your approval before it implements anything."
    } else {
        ""
    };
    let done_rule = if consultation.planning {
        "- If the agent has presented its plan and is waiting on your go-ahead, answer `done`:\n  \
         that approves the plan and lets it start implementing."
    } else {
        "- If the agent is reporting the task finished and is not waiting on you, the conversation\n  \
         is over: answer `done`."
    };
    let how_to_decide = [
        phase_note,
        "- If the agent asked you something you can answer, answer it. Prefer whatever it marked\n  \
         as recommended; failing that, the simplest option and the least work.",
        "- Never add requirements, never introduce facts you have not already stated, and never do\n  \
         the agent's work for it. No code, no file contents.",
        "- Keep it to a couple of sentences, as a person typing a reply would.",
        done_rule,
        "- If you genuinely cannot answer without inventing something, answer `cannot_answer`\n  \
         rather than guessing.",
    ]
    .into_iter()
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n");

    [
        "You are the person who asked for this work. An AI coding agent is doing the task and has",
        "stopped to say something. Decide what you say next.",
        "",
        "# What you originally asked for",
        "",
        consultation.task_prompt,
        "",
        // Folded into the heading rather than standing alone, so an absent
        // section leaves no hole in a file a person reads while auditing.
        &format!("{said_since}# What the agent just said"),
        "",
        consultation.final_message,
        "",
        "# How to decide",
        "",
        &how_to_decide,
        "",
        "# Task",
        "",
        &format!("{VERDICT_PATH_LINE} {}", verdict_path.display()),
        "",
        "The JSON must match this schema (exactly these keys, no extra prose in the file):",
        "",
        "```json",
        "{ \"verdict\": \"answer\"|\"done\"|\"cannot_answer\", \"reply\": \"what you say next\", \"rationale\": \"one line\" }",
        "```",
        "",
        "`reply` is required for `answer` and ignored otherwise.",
        "",
    ]
    .join("\n")
}

/// Read one verdict file. A fence is stripped first because models add one out
/// of habit, and stopping a run over punctuation would be a worse failure than
/// the three lines it costs to tolerate.
fn parse_verdict(raw: &str) -> Result<Verdict, ResponderStopCause> {
    let raw: RawVerdict =
        serde_json::from_str(unfence(raw)).map_err(|_| ResponderStopCause::MalformedVerdict)?;
    let rationale = raw
        .rationale
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty());
    match raw.verdict.as_str() {
        "answer" => Ok(Verdict::Answer {
            reply: raw.reply.unwrap_or_default(),
            rationale,
        }),
        "done" => Ok(Verdict::Done { rationale }),
        "cannot_answer" => Ok(Verdict::CannotAnswer { rationale }),
        _ => Err(ResponderStopCause::MalformedVerdict),
    }
}

/// Strip one wrapping code fence, if the whole body is inside it.
fn unfence(raw: &str) -> &str {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(body) = rest.split_once('\n').map(|(_language, body)| body) else {
        return trimmed;
    };
    body.trim_end()
        .strip_suffix("```")
        .unwrap_or(trimmed)
        .trim()
}

/// Decide whether a reply may be delivered. Every rejection stops the run,
/// which is the safe direction: an undelivered reply is a loud, greppable stop,
/// while a bad one enters the transcript as an ordinary user turn and is graded
/// as though the exchange really happened.
fn validate_reply(reply: &str, previous: Option<&str>) -> Result<(), ResponderStopCause> {
    let trimmed = reply.trim();
    if trimmed.is_empty() {
        return Err(ResponderStopCause::EmptyReply);
    }
    if reply.len() > MAX_REPLY_BYTES {
        return Err(ResponderStopCause::ReplyTooLong);
    }
    if reply.contains("```") {
        return Err(ResponderStopCause::ReplyContainsCode);
    }
    if previous.is_some_and(|previous| previous.trim() == trimmed) {
        return Err(ResponderStopCause::ReplyRepeated);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn consultation() -> Consultation<'static> {
        Consultation {
            task_prompt: "Requests to the pricing API are slow. Add caching.",
            prior_replies: &[],
            final_message: "Which cache should I use?",
            planning: false,
        }
    }

    /// The act-phase rules are what every responder eval has always been shown,
    /// so they are pinned verbatim: the planning variant must not drift them.
    #[test]
    fn the_act_phase_decision_rules_render_verbatim() {
        let prompt = build_prompt(&consultation(), Path::new("/w/v.json"));
        let expected = "\
# How to decide

- If the agent asked you something you can answer, answer it. Prefer whatever it marked
  as recommended; failing that, the simplest option and the least work.
- Never add requirements, never introduce facts you have not already stated, and never do
  the agent's work for it. No code, no file contents.
- Keep it to a couple of sentences, as a person typing a reply would.
- If the agent is reporting the task finished and is not waiting on you, the conversation
  is over: answer `done`.
- If you genuinely cannot answer without inventing something, answer `cannot_answer`
  rather than guessing.

# Task
";
        assert!(prompt.contains(expected), "{prompt}");
    }

    /// In the planning phase `done` approves the plan rather than ending the
    /// task, and the responder is told so — otherwise a presented plan reads
    /// like a question and gets a reply instead of a go-ahead.
    #[test]
    fn a_planning_consultation_tells_the_responder_what_done_means() {
        let planning = Consultation {
            planning: true,
            final_message: "Here is my plan: 1. add an LRU. 2. test it.",
            ..consultation()
        };
        let prompt = build_prompt(&planning, Path::new("/w/v.json"));
        assert!(prompt.contains("plan"), "{prompt}");
        assert!(prompt.contains("approve"), "{prompt}");
        assert!(prompt.contains("`done`"), "{prompt}");
        assert!(
            !prompt.contains("reporting the task finished"),
            "the act-phase meaning of done is not offered while planning: {prompt}"
        );

        let acting = build_prompt(&consultation(), Path::new("/w/v.json"));
        assert!(acting.contains("reporting the task finished"), "{acting}");
        assert!(!acting.contains("approve"), "{acting}");
    }

    #[test]
    fn the_prompt_carries_the_exchange_and_names_where_to_write() {
        let prompt = build_prompt(
            &consultation(),
            Path::new("/w/responder/turn-1/verdict.json"),
        );

        assert!(prompt.contains("Requests to the pricing API are slow. Add caching."));
        assert!(prompt.contains("Which cache should I use?"));
        assert!(
            prompt.contains(VERDICT_PATH_LINE),
            "the responder is told where to write: {prompt}"
        );
        assert!(prompt.contains("/w/responder/turn-1/verdict.json"));
    }

    /// A simulated user remembers what they already said, so a later round does
    /// not contradict an earlier answer.
    #[test]
    fn prior_replies_are_carried_into_the_prompt() {
        let replies = ["An in-process LRU is fine.".to_string()];
        let consultation = Consultation {
            prior_replies: &replies,
            ..consultation()
        };

        let prompt = build_prompt(&consultation, Path::new("/w/v.json"));
        assert!(prompt.contains("1. An in-process LRU is fine."));
        assert!(!prompt.contains("\n\n\n"), "{prompt}");
    }

    #[test]
    fn an_answer_verdict_parses_with_its_reply_and_rationale() {
        let raw =
            r#"{"verdict":"answer","reply":"Use the in-process LRU.","rationale":"simplest"}"#;

        let Verdict::Answer { reply, rationale } = parse_verdict(raw).unwrap() else {
            panic!("expected an answer");
        };
        assert_eq!(reply, "Use the in-process LRU.");
        assert_eq!(rationale.as_deref(), Some("simplest"));
    }

    #[test]
    fn a_done_verdict_parses_and_carries_no_reply() {
        let raw = r#"{"verdict":"done","rationale":"the agent reported the cache in place"}"#;

        let Verdict::Done { rationale } = parse_verdict(raw).unwrap() else {
            panic!("expected done");
        };
        assert_eq!(
            rationale.as_deref(),
            Some("the agent reported the cache in place")
        );
    }

    #[test]
    fn a_cannot_answer_verdict_parses() {
        let raw = r#"{"verdict":"cannot_answer","rationale":"it asked for a credential"}"#;

        assert!(matches!(
            parse_verdict(raw).unwrap(),
            Verdict::CannotAnswer { .. }
        ));
    }

    /// A rationale is a courtesy, not a contract: a verdict without one is
    /// still usable, and refusing it would stop a run over prose.
    #[test]
    fn a_verdict_without_a_rationale_is_still_usable() {
        let Verdict::Answer { rationale, .. } =
            parse_verdict(r#"{"verdict":"answer","reply":"Yes, go ahead."}"#).unwrap()
        else {
            panic!("expected an answer");
        };
        assert_eq!(rationale, None);
    }

    /// Models fence JSON out of habit. Stripping the fence costs three lines
    /// and saves a run that would otherwise stop over punctuation.
    #[test]
    fn a_fenced_verdict_is_unwrapped_before_parsing() {
        let raw = "```json\n{\"verdict\":\"done\"}\n```\n";

        assert!(matches!(parse_verdict(raw).unwrap(), Verdict::Done { .. }));
    }

    #[test]
    fn an_unknown_verdict_is_malformed() {
        assert_eq!(
            parse_verdict(r#"{"verdict":"maybe","reply":"hmm"}"#).unwrap_err(),
            ResponderStopCause::MalformedVerdict
        );
    }

    #[test]
    fn a_verdict_that_is_not_json_is_malformed() {
        assert_eq!(
            parse_verdict("I think you should use Redis.").unwrap_err(),
            ResponderStopCause::MalformedVerdict
        );
    }

    /// An `answer` with no reply is not an answer. Parsing yields an empty one
    /// so a single validation gate rejects it, rather than two paths deciding
    /// separately what "blank" means.
    #[test]
    fn an_answer_with_no_reply_parses_blank_and_fails_validation() {
        let Verdict::Answer { reply, .. } = parse_verdict(r#"{"verdict":"answer"}"#).unwrap()
        else {
            panic!("expected an answer");
        };
        assert_eq!(
            validate_reply(&reply, None).unwrap_err(),
            ResponderStopCause::EmptyReply
        );
    }

    #[test]
    fn a_whitespace_only_reply_is_empty() {
        assert_eq!(
            validate_reply("   \n\t ", None).unwrap_err(),
            ResponderStopCause::EmptyReply
        );
    }

    #[test]
    fn an_ordinary_reply_validates() {
        assert_eq!(validate_reply("Use the in-process LRU.", None), Ok(()));
    }

    /// A simulated user answers in sentences. A reply this long means the
    /// responder started doing the agent's work, and delivering it would put
    /// work into the transcript that the agent under test did not do.
    #[test]
    fn a_reply_over_the_byte_cap_is_rejected() {
        let long = "a".repeat(MAX_REPLY_BYTES + 1);

        assert_eq!(
            validate_reply(&long, None).unwrap_err(),
            ResponderStopCause::ReplyTooLong
        );
        assert_eq!(validate_reply(&"a".repeat(MAX_REPLY_BYTES), None), Ok(()));
    }

    #[test]
    fn a_reply_carrying_a_fenced_code_block_is_rejected() {
        let reply = "Sure, use this:\n\n```rust\nlet cache = Lru::new(128);\n```\n";

        assert_eq!(
            validate_reply(reply, None).unwrap_err(),
            ResponderStopCause::ReplyContainsCode
        );
    }

    /// The same answer twice means the exchange is circling. Spending the
    /// remaining turns on it would only cost more dispatches to reach the same
    /// place, so stop where it is legible.
    #[test]
    fn a_reply_identical_to_the_previous_one_is_rejected() {
        let reply = "Use the in-process LRU.";

        assert_eq!(
            validate_reply(reply, Some(reply)).unwrap_err(),
            ResponderStopCause::ReplyRepeated
        );
        assert_eq!(validate_reply(reply, Some("Use Redis.")), Ok(()));
    }

    /// Trailing whitespace is not a new answer.
    #[test]
    fn the_repeat_check_ignores_surrounding_whitespace() {
        assert_eq!(
            validate_reply("  Use the LRU.\n", Some("Use the LRU.")).unwrap_err(),
            ResponderStopCause::ReplyRepeated
        );
    }
}
