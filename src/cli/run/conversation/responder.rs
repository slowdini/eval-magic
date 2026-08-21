//! The heuristic responder: what a user would say next, decided mechanically.
//!
//! It reads one round's final assistant message as plain Markdown, which is why
//! it needs no harness-specific code — every harness's transcript parser
//! normalizes that text into `TranscriptSummary::final_text` already.

use std::sync::LazyLock;

use regex::Regex;

use crate::core::{ResponderAnswer, ResponderKind, ResponderRule, TurnOrigin};

/// What the heuristic made of one assistant turn.
pub(super) enum Reading {
    /// No question was asked — the agent considers the task done.
    NoQuestion,
    /// A question with no option list the heuristic can answer.
    Unanswerable,
    /// A mechanically answerable question, with the reply and its provenance.
    Answer { text: String, origin: TurnOrigin },
}

/// A list item: `- text`, `* text`, `+ text`, `1. text`, or `1) text`. Up to
/// three leading spaces, matching Markdown's own tolerance before a deeper
/// indent turns the line into a continuation of the item above it.
static OPTION_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}(?:[-*+]|\d{1,3}[.)])\s+(.*)$").unwrap());

/// A standalone `recommended`, in parentheses, brackets, or bold. Deliberately
/// narrow: an option that merely discusses what it recommends is not a marker.
/// A task-list marker at the head of an option body: `[ ]`, `[x]`, or `[X]`.
/// Its presence anywhere in a group is what makes the group multi-select.
static CHECKBOX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[( |x|X)\]\s*").unwrap());

static RECOMMENDED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\(\s*recommended\s*\)|\[\s*recommended\s*\]|\*\*\s*recommended\s*\*\*)")
        .unwrap()
});

/// One list of options, with the lines that introduced it.
struct OptionGroup {
    question: Option<String>,
    options: Vec<String>,
}

pub(super) fn read(final_message: &str) -> Reading {
    let lines: Vec<&str> = final_message.lines().collect();
    let answers: Vec<ResponderAnswer> = question_groups(&lines)
        .iter()
        .filter_map(answer_for)
        .collect();
    if answers.is_empty() {
        return if final_message.contains('?') {
            Reading::Unanswerable
        } else {
            Reading::NoQuestion
        };
    }
    Reading::Answer {
        text: render_reply(&answers),
        origin: TurnOrigin {
            responder: ResponderKind::Heuristic,
            answers,
        },
    }
}

/// Every option list in the message whose lead-in asks something.
fn question_groups(lines: &[&str]) -> Vec<OptionGroup> {
    let mut groups = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let Some(option) = option_body(lines[index]) else {
            index += 1;
            continue;
        };
        let start = index;
        let mut options = vec![option];
        index += 1;
        while index < lines.len() {
            if let Some(option) = option_body(lines[index]) {
                options.push(option);
                index += 1;
            } else if lines[index].trim().is_empty()
                && lines
                    .get(index + 1)
                    .copied()
                    .and_then(option_body)
                    .is_some()
            {
                // A loose Markdown list puts a blank line between its items.
                index += 1;
            } else {
                break;
            }
        }
        if options.len() < 2 {
            continue;
        }
        if let Some(question) = lead_in_question(lines, start) {
            groups.push(OptionGroup {
                question: Some(question),
                options,
            });
        }
    }
    groups
}

/// The text of a list item, or `None` when the line is not one.
fn option_body(line: &str) -> Option<String> {
    let body = OPTION_LINE.captures(line)?.get(1)?.as_str().trim();
    (!body.is_empty()).then(|| body.to_string())
}

/// The question a group hangs from: the last line of the contiguous non-blank
/// block directly above it, and only when that line *ends* with `?`.
///
/// Ending, not merely containing: a closing summary's list is introduced by a
/// line like `Here is what changed:`, and that line may well also carry a
/// rhetorical question earlier in the sentence. Answering such a list would
/// derail a finished task, so the `?` has to be the last thing said before the
/// options appear.
fn lead_in_question(lines: &[&str], start: usize) -> Option<String> {
    let mut end = start;
    while end > 0 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    if end == 0 || option_body(lines[end - 1]).is_some() {
        return None;
    }
    let question = strip_emphasis(lines[end - 1].trim());
    question.ends_with('?').then(|| question.to_string())
}

fn strip_emphasis(text: &str) -> &str {
    text.trim_matches(|c| c == '*' || c == '_' || c == '`' || c == '#')
        .trim()
}

/// Apply the selection rules to one group.
fn answer_for(group: &OptionGroup) -> Option<ResponderAnswer> {
    // Checkbox syntax is the only mechanical signal that a question takes zero
    // or more answers rather than exactly one.
    let multi_select = group.options.iter().any(|option| CHECKBOX.is_match(option));
    let recommended: Vec<String> = group
        .options
        .iter()
        .filter(|option| is_recommended(option))
        .map(|option| clean(option))
        .collect();
    let (rule, chosen) = match (recommended.is_empty(), multi_select) {
        (false, true) => (ResponderRule::RecommendedOption, recommended),
        (false, false) => (
            ResponderRule::RecommendedOption,
            vec![recommended[0].clone()],
        ),
        (true, true) => (ResponderRule::NoSelection, Vec::new()),
        (true, false) => (ResponderRule::FirstOption, vec![clean(&group.options[0])]),
    };
    Some(ResponderAnswer {
        question: group.question.clone(),
        options: group.options.clone(),
        rule,
        chosen,
    })
}

/// A marked recommendation, or a pre-checked box — the plainest statement of a
/// suggested default a Markdown list can carry.
fn is_recommended(option: &str) -> bool {
    RECOMMENDED.is_match(option)
        || CHECKBOX
            .captures(option)
            .and_then(|caps| caps.get(1))
            .is_some_and(|marker| marker.as_str() != " ")
}

/// An option as a user would say it back: markers stripped, spacing tidied.
fn clean(option: &str) -> String {
    RECOMMENDED
        .replace_all(&CHECKBOX.replace(option, ""), "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['-', '\u{2014}', ':', ','])
        .trim()
        .to_string()
}

/// What the responder says when a checkbox question recommends nothing. A user
/// still has to answer, and "nothing" is the answer the rules produced.
const NO_SELECTION_REPLY: &str = "None of these.";

fn render_reply(answers: &[ResponderAnswer]) -> String {
    answers
        .iter()
        .enumerate()
        .map(|(index, answer)| {
            let chosen = match answer.chosen.is_empty() {
                true => NO_SELECTION_REPLY.to_string(),
                false => answer.chosen.join(", "),
            };
            // A single answer needs no ordinal; several do, so the agent can
            // tell which reply belongs to which question it asked.
            match answers.len() {
                1 => chosen,
                _ => format!("{}. {chosen}", index + 1),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{Reading, read};
    use crate::core::ResponderRule;

    /// The happy path #244 names: an option marked as recommended is the answer.
    #[test]
    fn a_recommended_option_is_chosen() {
        let message = "\
I can add caching two ways. Which do you want?

- Use an in-process LRU cache (Recommended)
- Add Redis
";

        let Reading::Answer { text, origin } = read(message) else {
            panic!("a recommended option must be answerable");
        };

        assert_eq!(text, "Use an in-process LRU cache");
        assert_eq!(origin.answers.len(), 1);
        assert_eq!(origin.answers[0].rule, ResponderRule::RecommendedOption);
        assert_eq!(origin.answers[0].chosen, ["Use an in-process LRU cache"]);
    }

    /// Exactly one choice is required and none is recommended, so the list's
    /// first option wins. Mechanical, not a judgement about which is better.
    #[test]
    fn a_plain_list_with_no_recommendation_takes_the_first_option() {
        let message = "\
Which database should I target?

1. PostgreSQL
2. MySQL
3. SQLite
";

        let Reading::Answer { text, origin } = read(message) else {
            panic!("a plain option list is answerable");
        };

        assert_eq!(text, "PostgreSQL");
        assert_eq!(origin.answers[0].rule, ResponderRule::FirstOption);
        assert_eq!(origin.answers[0].chosen, ["PostgreSQL"]);
    }

    /// A checkbox list asks for zero or more. With nothing recommended, zero is
    /// the mechanical answer — the responder does not invent a preference.
    #[test]
    fn a_checkbox_list_with_no_recommendation_selects_nothing() {
        let message = "\
Which extras should I include?

- [ ] Unit tests
- [ ] Integration tests
- [ ] Benchmarks
";

        let Reading::Answer { text, origin } = read(message) else {
            panic!("a checkbox list is answerable");
        };

        assert_eq!(text, "None of these.");
        assert_eq!(origin.answers[0].rule, ResponderRule::NoSelection);
        assert!(origin.answers[0].chosen.is_empty());
    }

    /// Zero or more means every recommendation can be taken, unlike a
    /// single-choice list where only the first can.
    #[test]
    fn a_checkbox_list_selects_every_recommended_option() {
        let message = "\
Which extras should I include?

- [ ] Unit tests (Recommended)
- [ ] Integration tests
- [ ] Docs (Recommended)
";

        let Reading::Answer { text, origin } = read(message) else {
            panic!("a checkbox list is answerable");
        };

        assert_eq!(text, "Unit tests, Docs");
        assert_eq!(origin.answers[0].rule, ResponderRule::RecommendedOption);
        assert_eq!(origin.answers[0].chosen, ["Unit tests", "Docs"]);
    }

    /// A pre-checked box is the plainest possible statement of a suggested
    /// default, so it reads as a recommendation.
    #[test]
    fn a_pre_checked_box_counts_as_a_recommendation() {
        let message = "\
Which extras should I include?

- [ ] Unit tests
- [x] Docs
";

        let Reading::Answer { text, origin } = read(message) else {
            panic!("a checkbox list is answerable");
        };

        assert_eq!(text, "Docs");
        assert_eq!(origin.answers[0].rule, ResponderRule::RecommendedOption);
    }

    /// A message may ask more than one thing. Each list is answered under its
    /// own rule, and the reply is numbered so the agent can tell them apart.
    #[test]
    fn two_option_groups_are_answered_in_order() {
        let message = "\
A couple of decisions before I start.

Which database?

- PostgreSQL (Recommended)
- MySQL

Which extras do you want?

- [ ] Benchmarks
- [ ] Fuzzing
";

        let Reading::Answer { text, origin } = read(message) else {
            panic!("two option lists are answerable");
        };

        assert_eq!(text, "1. PostgreSQL\n2. None of these.");
        assert_eq!(origin.answers.len(), 2);
        assert_eq!(origin.answers[0].rule, ResponderRule::RecommendedOption);
        assert_eq!(origin.answers[1].rule, ResponderRule::NoSelection);
        assert_eq!(
            origin.answers[1].question.as_deref(),
            Some("Which extras do you want?")
        );
    }

    /// The load-bearing negative: a closing summary is mostly a bulleted list,
    /// and answering one as if it were a question would derail a finished task.
    /// Requiring the lead-in to ask something is what separates the two.
    #[test]
    fn a_closing_summary_with_a_bulleted_list_is_not_a_question() {
        let message = "\
Done. I made these changes:

- Fixed the date parser
- Added a regression test
- Updated the changelog
";

        assert!(matches!(read(message), Reading::NoQuestion));
    }

    /// A question with no options is the branch the LLM responder takes over.
    /// Until then it stops the run rather than inventing an answer.
    #[test]
    fn a_free_form_question_is_unanswerable() {
        let message = "Before I start — what should happen to rows with a null created_at?";

        assert!(matches!(read(message), Reading::Unanswerable));
    }

    /// Completion detection: no question at all means the agent is done, so the
    /// conversation ends instead of burning its remaining turns.
    #[test]
    fn a_message_with_no_question_reads_as_done() {
        let message = "Caching is in place and the pricing endpoint is under 40ms.";

        assert!(matches!(read(message), Reading::NoQuestion));
    }

    /// Removing a trailing marker can leave the separator that introduced it
    /// dangling, and echoing "Use an in-process LRU —" back at the agent reads
    /// as a truncated thought.
    #[test]
    fn a_separator_left_by_a_trailing_marker_is_cleaned_up() {
        let message = "\
Which cache should I use?

- An in-process LRU — (Recommended)
- Redis
";

        let Reading::Answer { text, .. } = read(message) else {
            panic!("a recommended option must be answerable");
        };

        assert_eq!(text, "An in-process LRU");
    }

    /// A deliberate, documented conservatism: a stray question mark anywhere in
    /// an otherwise-finished message stops the run instead of calling it done.
    /// Stopping wastes a dispatch; guessing "complete" while the agent waits
    /// would record a half-finished run as data.
    #[test]
    fn a_stray_question_mark_stops_rather_than_claiming_completion() {
        let message = "\
Why a decorator? It keeps the call sites untouched. Here is what changed:

- Wrapped the client
- Added the cache
";

        assert!(matches!(read(message), Reading::Unanswerable));
    }
}
