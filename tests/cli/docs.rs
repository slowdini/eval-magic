//! Embedded user guides shipped in the binary.
//!
//! Every Markdown file directly under `docs/guides/` is a CLI topic. The file
//! stem is its topic name, the first H1 is its listing title, and the complete
//! source body is printed verbatim.

use crate::helpers::skill_eval;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use std::path::{Path, PathBuf};

mod codebase;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn guide_sources() -> Vec<(String, String, String, PathBuf)> {
    let guide_dir = repo_root().join("docs/guides");
    let mut guides = fs::read_dir(&guide_dir)
        .unwrap_or_else(|err| panic!("expected guide directory {}: {err}", guide_dir.display()))
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            (path.extension().and_then(|value| value.to_str()) == Some("md")).then_some(path)
        })
        .map(|path| {
            let topic = path.file_stem().unwrap().to_str().unwrap().to_string();
            let body = fs::read_to_string(&path).unwrap();
            let title = body
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("# "))
                .unwrap_or_else(|| panic!("{} must start with an H1", path.display()))
                .to_string();
            (topic, title, body, path)
        })
        .collect::<Vec<_>>();
    guides.sort_by(|left, right| left.0.cmp(&right.0));
    guides
}

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
        .filter(|line| line.starts_with("  "))
        .filter_map(|line| line.trim().split_once(char::is_whitespace))
        .map(|(name, _)| name.to_string())
        .collect()
}

#[test]
fn docs_listing_matches_the_guide_directory() {
    let guides = guide_sources();
    let expected = guides
        .iter()
        .map(|(topic, _, _, _)| topic.clone())
        .collect::<Vec<_>>();
    assert_eq!(listed_topics(), expected);

    let output = skill_eval()
        .arg("docs")
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let topic_width = guides
        .iter()
        .map(|(topic, _, _, _)| topic.len())
        .max()
        .unwrap_or_default();
    for (topic, title, _, _) in guides {
        let row = format!("  {topic:topic_width$} {title}");
        assert!(stdout.lines().any(|line| line == row), "{stdout}");
    }
}

#[test]
fn docs_prints_every_guide_verbatim() {
    for (topic, _, body, path) in guide_sources() {
        let output = skill_eval()
            .args(["docs", &topic])
            .assert()
            .success()
            .get_output()
            .clone();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            body,
            "topic {topic} drifted from {}",
            path.display()
        );
    }
}

#[test]
fn docs_guide_topic_is_retired() {
    skill_eval()
        .args(["docs", "guide"])
        .assert()
        .failure()
        .stderr(
            contains("unknown docs topic 'guide'")
                .and(contains("byoh"))
                .and(contains("isolation")),
        );
}

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

#[test]
fn docs_byoh_keeps_the_authoring_workflow() {
    skill_eval()
        .args(["docs", "byoh"])
        .assert()
        .success()
        .stdout(contains("# Bring your own harness"))
        .stdout(contains("harness init"))
        .stdout(contains("harness lint"))
        .stdout(contains("--probe"))
        .stdout(contains("additional_project_skill_dirs"))
        .stdout(contains("[plan_mode]"))
        .stdout(contains("{mode_args}"))
        .stdout(contains("Upstreaming your descriptor"));
}

#[test]
fn docs_conversations_keeps_the_plan_mode_contract() {
    skill_eval()
        .args(["docs", "conversations"])
        .assert()
        .success()
        .stdout(contains("# Multi-turn conversations"))
        .stdout(contains("Starting in plan mode"))
        .stdout(contains("\"plan_mode\": true"))
        .stdout(contains("plan-mode"))
        .stdout(contains("The plan is approved. Implement it now."))
        .stdout(contains("plan_file"))
        .stdout(contains("plan_not_presented"))
        .stdout(contains("plan_approval"))
        .stdout(contains("plan_mode_attributed"))
        .stdout(contains("plan.md"));
}

#[test]
fn docs_isolation_keeps_remedies_and_verification() {
    skill_eval()
        .args(["docs", "isolation"])
        .assert()
        .success()
        .stdout(contains("# Dispatch isolation and sandbox boundaries"))
        .stdout(contains("--setting-sources project,local"))
        .stdout(contains("CLAUDE_CONFIG_DIR"))
        .stdout(contains("--disable plugins"))
        .stdout(contains("OPENCODE_DISABLE_EXTERNAL_SKILLS"))
        .stdout(contains("resumed"))
        .stdout(contains("isolates_live_sources"))
        .stdout(contains("operator-environment"))
        .stdout(contains("codebase-sourced"))
        .stdout(contains("codebase.exclude_skill_sources"))
        .stdout(contains("claude plugin list"))
        .stdout(contains("`comparison-invalid`"))
        .stdout(contains("\"subtype\":\"init\""));
}

#[test]
fn docs_isolation_explains_nested_codex_sandboxes_and_remedies() {
    skill_eval()
        .args(["docs", "isolation"])
        .assert()
        .success()
        .stdout(contains("**Operator sandbox**"))
        .stdout(contains("**Harness task sandbox**"))
        .stdout(contains("**Eval guard**"))
        .stdout(contains("**Skill-source isolation**"))
        .stdout(contains("same generated task command"))
        .stdout(contains("equivalent inputs and configuration"))
        .stdout(contains("fails inside the operator Codex session"))
        .stdout(contains("Operation not permitted"))
        .stdout(contains("alone does not establish"))
        .stdout(contains("1. **Preferred:**"))
        .stdout(contains("ordinary terminal"))
        .stdout(contains("2. **Alternative:**"))
        .stdout(contains("outer launch of `eval-magic dispatch`"))
        .stdout(contains("surface and policy support"))
        .stdout(contains("Adding a writable directory alone"))
        .stdout(contains("--sandbox workspace-write"))
        .stdout(contains("eval guard enabled"));

    skill_eval()
        .args(["docs", "--help"])
        .assert()
        .success()
        .stdout(contains("sandbox boundaries"))
        .stdout(contains("live skill sources"));
}

#[test]
fn docs_guard_keeps_configuration_defaults_and_boundary_contracts() {
    skill_eval()
        .args(["docs", "guard"])
        .assert()
        .success()
        .stdout(contains("# Configuring guarded commands"))
        .stdout(contains("allow_tools"))
        .stdout(contains("allow_commands"))
        .stdout(contains("language/rust"))
        .stdout(contains("framework/nextjs"))
        .stdout(contains("replaces"))
        .stdout(contains("dispatch.json"))
        .stdout(contains("guard_armed"))
        .stdout(contains("unknown"))
        .stdout(contains("cannot override"));

    skill_eval()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(contains("eval-magic docs guard"))
        .stdout(contains("guard_armed"));
}

/// Judge evidence is the primary grading input, so the shipped reference must
/// keep the bounds, sampling semantics, trust boundary, source fallback, and
/// retention contract.
#[test]
fn docs_judging_keeps_bundle_bounds_truncation_and_retention_contract() {
    skill_eval()
        .args(["docs", "judging"])
        .assert()
        .success()
        .stdout(contains("# Judge evidence bundles"))
        .stdout(contains("judge-evidence.md"))
        .stdout(contains("98,304 bytes"))
        .stdout(contains("131,072 bytes"))
        .stdout(contains("diff.patch"))
        .stdout(contains("run.json"))
        .stdout(contains("final_message"))
        .stdout(contains("conversation transcript"))
        .stdout(contains("tool invocation summary"))
        .stdout(contains("truncated"))
        .stdout(contains("untrusted"))
        .stdout(contains("read-only"))
        .stdout(contains("\"samples\": 10"))
        .stdout(contains("--judge-samples"))
        .stdout(contains("6 / 10"))
        .stdout(contains("0.6^10"))
        .stdout(contains("__sample-N"))
        .stdout(contains("missing response"))
        .stdout(contains("__skill_invoked"))
        .stdout(contains("Explore before writing assertions"))
        .stdout(contains("eval-magic compare"))
        .stdout(contains("no assertions"))
        .stdout(contains("not a grade"))
        .stdout(contains("evals/baseline/evidence"));

    // The explore-first loop is only usable if it names the file to edit and
    // what re-reads it (#295).
    skill_eval()
        .args(["docs", "judging"])
        .assert()
        .success()
        .stdout(contains("Which evals.json grade reads"))
        .stdout(contains("evals/evals.json"))
        .stdout(contains("skill_should_trigger"))
        .stdout(contains("assertion_source"))
        .stdout(contains("eval-magic grade --overwrite"))
        .stdout(contains("exact `run.json` digest"))
        .stdout(contains("must not be resumed"))
        .stdout(contains("eval-magic dispatch --judges --overwrite"));

    // The isolation guide explains why the copy exists, so it has to carry the
    // one exception rather than contradict the judging guide.
    skill_eval()
        .args(["docs", "isolation"])
        .assert()
        .success()
        .stdout(contains("freezes the treatment, not the assertions"))
        .stdout(contains("eval-magic docs judging"));

    skill_eval()
        .args(["grade", "--help"])
        .assert()
        .success()
        .stdout(contains("assertion_source"));

    skill_eval()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(contains("--judge-samples"))
        .stdout(contains("pass^k"));
}

#[test]
fn shipped_guides_do_not_depend_on_repository_relative_links() {
    for (topic, _, body, path) in guide_sources() {
        for destination in body
            .match_indices("](")
            .map(|(index, _)| &body[index + 2..])
            .filter_map(|rest| rest.split_once(')').map(|(destination, _)| destination))
        {
            assert!(
                destination.starts_with("https://") || destination.starts_with('#'),
                "topic {topic} has a repository-relative link `{destination}` in {}",
                path.display()
            );
        }
    }
}

#[test]
fn docs_unknown_topic_lists_every_available_guide() {
    let assert = skill_eval().args(["docs", "nope"]).assert().failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    for (topic, _, _, _) in guide_sources() {
        assert!(stderr.contains(&topic), "{stderr}");
    }
}

#[test]
fn harness_authoring_help_points_at_the_byoh_guide() {
    for args in ["--help", "harness init --help"] {
        skill_eval()
            .args(args.split_whitespace())
            .assert()
            .success()
            .stdout(contains("eval-magic docs byoh"));
    }
}

#[test]
fn every_guide_reference_in_shipped_help_resolves() {
    let topics = listed_topics();
    for help_args in [
        "--help",
        "run --help",
        "dispatch --help",
        "snapshot --help",
        "teardown --help",
        "teardown-guard --help",
        "ingest --help",
        "finalize --help",
        "record-runs --help",
        "detect-stray-writes --help",
        "grade --help",
        "aggregate --help",
        "init --help",
        "promote-baseline --help",
        "validate --help",
        "harness --help",
        "harness init --help",
        "harness list --help",
        "harness show --help",
        "harness lint --help",
        "docs --help",
    ] {
        let output = skill_eval()
            .args(help_args.split_whitespace())
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        for rest in stdout
            .match_indices("eval-magic docs ")
            .map(|(index, _)| &stdout[index + "eval-magic docs ".len()..])
            .filter(|rest| {
                rest.starts_with(|character: char| {
                    character.is_ascii_alphanumeric() || character == '-'
                })
            })
        {
            let topic = rest
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric() || *character == '-')
                .collect::<String>();
            assert!(
                topics.contains(&topic),
                "`{help_args}` references `eval-magic docs {topic}`, but topics are {topics:?}"
            );
        }
    }
}

#[test]
fn repository_documentation_map_names_each_surface() {
    let overview_path = repo_root().join("docs/developer_overview.md");
    let overview = fs::read_to_string(&overview_path)
        .unwrap_or_else(|err| panic!("expected {}: {err}", overview_path.display()));

    for heading in [
        "## How an evaluation moves through the system",
        "## Repository map",
        "## Sources of truth",
        "## Documentation policy",
        "## Internal guide index",
    ] {
        assert!(overview.contains(heading), "missing {heading}");
    }

    assert!(
        !repo_root().join("docs/README.md").exists(),
        "the developer overview replaces docs/README.md"
    );

    let agents = fs::read_to_string(repo_root().join("AGENTS.md")).unwrap();
    assert!(agents.contains("docs/guides/"));
    assert!(agents.contains("docs/developer_overview.md"));
    assert!(!agents.contains("docs/README.md"));

    // A POSIX shell is a development requirement, not a probed capability: the
    // dispatch tests spawn a `#!/bin/sh` stub through it and cannot skip. Both
    // contributor-facing docs have to say so, including where Windows
    // contributors run the toolchain.
    for (name, text) in [("AGENTS.md", &agents), ("developer overview", &overview)] {
        assert!(
            text.contains("POSIX shell"),
            "{name} should record the POSIX shell development requirement"
        );
        assert!(
            text.contains("WSL"),
            "{name} should direct Windows work to WSL"
        );
        assert!(
            text.contains("native Windows"),
            "{name} should state the unsupported native-Windows boundary"
        );
        assert!(
            !text.contains("Git Bash"),
            "{name} should not retain a Git Bash fallback"
        );
    }
}

/// `--help` is the primary discovery surface, so the host requirement is
/// reachable there without installing anything or preparing a run first.
#[test]
fn help_states_the_posix_tooling_requirement() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("REQUIREMENTS:"))
        .stdout(contains("POSIX shell"))
        .stdout(contains("WSL"))
        .stdout(contains("native Windows"))
        .stdout(contains("Git Bash").not())
        .stdout(contains("PowerShell").not())
        // `jq` was a requirement only while operators pasted the generated
        // recipes; the runner dispatches directly and needs no such toolchain.
        .stdout(contains("jq").not());
}

#[test]
fn readme_is_a_concise_first_run_path() {
    let readme = fs::read_to_string(repo_root().join("README.md")).unwrap();

    for expected in [
        "## Install",
        "## Quickstart",
        "cargo install eval-magic",
        "eval-magic init",
        "eval-magic run",
        "eval-magic snapshot --label baseline --ref HEAD",
        "RUNBOOK.md",
        "eval-magic teardown",
        "eval-magic docs byoh",
        "eval-magic docs isolation",
        "docs/developer_overview.md",
        // The declared host requirement, stated for both audiences the README
        // serves: installing the tool and developing it. `jq` is deliberately
        // absent because the runner dispatches directly.
        "POSIX shell",
        "WSL",
        "native Windows",
    ] {
        assert!(readme.contains(expected), "README is missing {expected}");
    }

    for retired in ["Git Bash", "Windows PowerShell", "eval-magic-installer.ps1"] {
        assert!(!readme.contains(retired), "README still contains {retired}");
    }

    assert!(
        readme.lines().count() <= 175,
        "README should hand detail to shipped docs instead of duplicating it"
    );
    assert!(!readme.contains("## Harnesses"));
    assert!(!readme.contains("docs/README.md"));
}

#[test]
fn readme_opens_with_live_project_branding() {
    let readme = fs::read_to_string(repo_root().join("README.md")).unwrap();
    let banner = r#"<img src="assets/readme.png""#;
    let banner_position = readme
        .find(banner)
        .expect("README should render the banner");
    let title_position = readme
        .find("# eval-magic")
        .expect("README should retain its text title");

    assert!(
        banner_position < title_position,
        "the banner should appear before the text title"
    );

    for expected in [
        r#"alt="eval-magic — Prove your skills actually work with structured, iterative eval loops""#,
        "https://img.shields.io/github/actions/workflow/status/slowdini/eval-magic/ci.yml?branch=dev",
        "https://codecov.io/gh/slowdini/eval-magic/branch/dev/graph/badge.svg",
        "https://img.shields.io/github/v/release/slowdini/eval-magic",
        "https://img.shields.io/crates/v/eval-magic",
        "https://img.shields.io/github/license/slowdini/eval-magic",
        "https://github.com/slowdini/eval-magic/actions/workflows/ci.yml",
        "https://app.codecov.io/gh/slowdini/eval-magic",
        "https://github.com/slowdini/eval-magic/releases/latest",
        "https://crates.io/crates/eval-magic",
        r#"href="./LICENSE""#,
    ] {
        assert!(readme.contains(expected), "README is missing {expected}");
    }
}
