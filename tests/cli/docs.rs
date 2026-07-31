//! `docs` — the embedded user-facing reference docs shipped in the binary.
//!
//! The binary is the only doc surface an installer-script user has locally, so
//! every user-facing reference doc is embedded and printable, and every
//! `eval-magic docs <topic>` mention in shipped output must name a real topic.

use crate::helpers::skill_eval;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

/// The topic names printed by a bare `eval-magic docs` invocation.
fn listed_topics() -> Vec<String> {
    let output = skill_eval()
        .arg("docs")
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    stdout
        .lines()
        // Topic rows are indented "<name>  <summary>"; the header is not.
        .filter(|line| line.starts_with("  "))
        .filter_map(|line| line.trim().split_once(char::is_whitespace))
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Bare `docs` lists every embedded topic with a summary.
#[test]
fn docs_lists_topics() {
    skill_eval()
        .arg("docs")
        .assert()
        .success()
        .stdout(contains("guide"))
        .stdout(contains("byoh"))
        .stdout(contains("operating guide"))
        .stdout(contains("harness"));
}

/// `docs guide` prints the embedded operating guide (the README body).
#[test]
fn docs_guide_prints_operating_guide() {
    skill_eval()
        .args(["docs", "guide"])
        .assert()
        .success()
        .stdout(contains("# eval-magic"))
        .stdout(contains("## Quickstart"))
        .stdout(contains("## Reading results"));
}

/// `docs byoh` prints the embedded bring-your-own-harness authoring guide.
#[test]
fn docs_byoh_prints_embedded_authoring_guide() {
    skill_eval()
        .args(["docs", "byoh"])
        .assert()
        .success()
        .stdout(contains("# Bring your own harness"))
        .stdout(contains("exec_template"))
        .stdout(contains("Upstreaming your descriptor"));
}

/// An unknown topic fails and names the available topics.
#[test]
fn docs_unknown_topic_fails_listing_available() {
    skill_eval()
        .args(["docs", "nope"])
        .assert()
        .failure()
        .stderr(
            contains("unknown docs topic 'nope'")
                .and(contains("guide"))
                .and(contains("byoh")),
        );
}

/// The top-level EXAMPLES block points BYOH readers at the embedded docs.
#[test]
fn top_level_help_examples_point_at_docs_subcommand() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("eval-magic docs byoh"));
}

/// `harness init --help` points at the embedded docs, not a repo-relative path
/// a binary-only user cannot open.
#[test]
fn harness_init_help_points_at_docs_subcommand() {
    skill_eval()
        .args(["harness", "init", "--help"])
        .assert()
        .success()
        .stdout(contains("eval-magic docs byoh"));
}

/// Drift guard: every `eval-magic docs <topic>` mention in shipped help output
/// names a topic the binary actually embeds.
#[test]
fn shipped_help_references_resolve_to_real_topics() {
    let topics = listed_topics();
    assert!(topics.len() >= 2, "bare `docs` lists topics: {topics:?}");

    for help_args in ["--help", "harness init --help", "run --help"] {
        let output = skill_eval()
            .args(help_args.split_whitespace())
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let references = stdout
            .match_indices("eval-magic docs ")
            .map(|(i, _)| &stdout[i + "eval-magic docs ".len()..]);
        for rest in references {
            let referenced: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            assert!(
                topics.contains(&referenced),
                "`{help_args}` references `eval-magic docs {referenced}`, \
                 but bare `docs` lists only: {topics:?}"
            );
        }
    }
}
