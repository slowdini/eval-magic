//! `docs` — print the embedded user-facing reference docs.
//!
//! The installed binary is the only doc surface an installer-script user has
//! locally, so every user-facing reference doc is embedded at compile time
//! (version-matched, offline). Internal contributor docs stay unembedded in
//! the repository's `docs/` directory — see docs/README.md for the placement
//! policy.

use anyhow::bail;

/// One embedded documentation topic: its CLI name, a one-line summary for the
/// topic listing, and the full embedded body.
struct Topic {
    name: &'static str,
    summary: &'static str,
    body: &'static str,
}

/// The shipped reference docs. References from help text and console output
/// use `eval-magic docs <name>`; `tests/cli/docs.rs` drift-guards that every
/// such mention names a topic listed here.
const TOPICS: &[Topic] = &[
    Topic {
        name: "guide",
        summary: "complete operating guide: install, the run loop, assertions, results",
        body: include_str!("../../../README.md"),
    },
    Topic {
        name: "byoh",
        summary: "author a TOML descriptor for a harness eval-magic has never seen",
        body: include_str!("../../../docs/byoh.md"),
    },
    Topic {
        name: "isolation",
        summary: "isolate dispatches from live/installed skills, and verify it worked",
        body: include_str!("../../../docs/isolation.md"),
    },
];

/// The bare-`docs` listing: one indented `<name>  <summary>` row per topic.
///
/// The name column is padded to the longest registered name rather than a
/// literal width, so adding a topic can't silently misalign the listing — the
/// first surface a new user reads.
fn render_topic_list() -> String {
    let mut out =
        String::from("Embedded reference docs — print one with `eval-magic docs <topic>`:\n\n");
    let width = TOPICS
        .iter()
        .map(|topic| topic.name.len())
        .max()
        .unwrap_or(0);
    for topic in TOPICS {
        out.push_str(&format!("  {:<width$} {}\n", topic.name, topic.summary));
    }
    out
}

/// Print one topic's body, or the topic listing when no topic is named.
pub(crate) fn run_docs(topic: Option<String>) -> anyhow::Result<()> {
    match topic {
        None => print!("{}", render_topic_list()),
        Some(name) => {
            let Some(topic) = TOPICS.iter().find(|topic| topic.name == name) else {
                let known = TOPICS
                    .iter()
                    .map(|topic| topic.name)
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!("unknown docs topic '{name}' (available: {known})");
            };
            print!("{}", topic.body);
        }
    }
    Ok(())
}
