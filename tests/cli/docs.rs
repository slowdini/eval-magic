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
        .stdout(contains("isolation"))
        .stdout(contains("operating guide"))
        .stdout(contains("harness"));
}

/// Every listing row fits an 80-column terminal. `docs/README.md` treats "the
/// bare-`docs` listing stops fitting on a screen" as a trigger for moving to
/// hosted docs, so a wrapped row would misreport that threshold as reached.
#[test]
fn docs_listing_rows_fit_eighty_columns() {
    let output = skill_eval()
        .arg("docs")
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    for line in stdout.lines() {
        assert!(
            line.chars().count() <= 80,
            "listing row wraps at 80 columns ({} chars): {line}",
            line.chars().count()
        );
    }
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
        .stdout(contains("## Reading results"))
        .stdout(contains("verdicts present"))
        .stdout(contains("exits nonzero"));
}

#[test]
fn docs_guide_explains_per_assertion_benchmark_counts() {
    skill_eval()
        .args(["docs", "guide"])
        .assert()
        .success()
        .stdout(contains("\"assertions\""))
        .stdout(contains("observed assertion results"))
        .stdout(contains("meta-results"));
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

/// `docs isolation` prints the embedded live-source isolation guide. One anchor
/// per harness, so a section cannot quietly vanish and leave the topic claiming
/// coverage it no longer has.
#[test]
fn docs_isolation_prints_embedded_topic() {
    skill_eval()
        .args(["docs", "isolation"])
        .assert()
        .success()
        .stdout(contains("# Isolating dispatches from live skill sources"))
        .stdout(contains("--setting-sources project,local"))
        .stdout(contains("CLAUDE_CONFIG_DIR"))
        .stdout(contains("--disable plugins"))
        .stdout(contains("OPENCODE_DISABLE_EXTERNAL_SKILLS"))
        .stdout(contains("isolates_live_sources"));
}

/// The topic must keep naming the tool that *cannot* answer "did this dispatch
/// load the plugin", and the event that can. Both cost real debugging time to
/// discover (issue #207), and both read as trimmable detail to a future editor.
#[test]
fn docs_isolation_documents_the_plugin_list_antipattern() {
    skill_eval()
        .args(["docs", "isolation"])
        .assert()
        .success()
        .stdout(contains("claude plugin list"))
        .stdout(contains("\"subtype\":\"init\""));
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
                .and(contains("byoh"))
                .and(contains("isolation")),
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
    assert!(topics.len() >= 3, "bare `docs` lists topics: {topics:?}");

    for help_args in [
        "--help",
        "harness init --help",
        "run --help",
        "aggregate --help",
    ] {
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
