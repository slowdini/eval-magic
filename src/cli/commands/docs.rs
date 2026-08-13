//! `docs` — print the embedded user-facing reference docs.
//!
//! The installed binary is the only doc surface an installer-script user has
//! locally, so every Markdown file directly under `docs/guides/` is discovered and
//! embedded at compile time. Internal contributor docs stay unembedded in the
//! repository's `docs/` root — see `docs/developer_overview.md`.

use anyhow::bail;

/// One embedded documentation topic: its CLI name, a one-line summary for the
/// topic listing, and the full embedded body.
struct Topic {
    name: &'static str,
    summary: &'static str,
    body: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/guide_topics.rs"));

/// The bare-`docs` listing: one indented `<name>  <summary>` row per topic.
///
/// The name column is padded to the longest registered name rather than a
/// literal width, so adding a topic cannot silently misalign the listing.
fn render_topic_list() -> String {
    let mut out = String::from("Shipped guides — print one with `eval-magic docs <topic>`:\n\n");
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
