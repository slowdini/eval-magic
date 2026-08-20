//! Help output, `validate`, and parser-level dispatch (unknown subcommands).

use crate::helpers::skill_eval;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::fs;
use tempfile::TempDir;

/// A minimal valid `evals.json` body.
const VALID_EVALS: &str = r#"{ "skill_name": "demo", "evals": [
    { "id": "e1", "prompt": "p", "expected_output": "o" } ] }"#;

/// Build `<root>/<skill>/evals/evals.json` with the given contents.
fn write_evals(root: &std::path::Path, skill: &str, contents: &str) {
    let dir = root.join(skill).join("evals");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("evals.json"), contents).unwrap();
}

/// The hidden `__fixture` subcommand is the suite's stand-in for `sh`, `true`,
/// and `printf`, so its wiring — parsing, effects, exit code, and stream
/// flushing before `process::exit` — has to hold end to end, not just in the
/// unit tests that drive it with in-memory buffers.
#[test]
fn hidden_fixture_subcommand_emits_and_exits() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("written.txt");

    skill_eval()
        .args([
            "__fixture",
            "--exit",
            "3",
            "--text",
            "hello world",
            "--stderr",
            "diagnostic",
        ])
        .arg("--write")
        .arg(&target)
        .assert()
        .code(3)
        .stdout("hello world")
        .stderr("diagnostic");

    assert_eq!(fs::read_to_string(&target).unwrap(), "hello world");
}

/// Hidden means hidden: `__fixture` must never surface in the help tree a user
/// reads, the way `guard-hook` does not.
#[test]
fn hidden_fixture_subcommand_is_absent_from_help() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("__fixture").not());
}

/// `--help` succeeds and lists the subcommands.
#[test]
fn help_lists_subcommands() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("init"))
        .stdout(contains("record-runs"))
        .stdout(contains("grade"))
        .stdout(contains("validate"))
        .stdout(contains("aggregate"));
}

/// The binary name in help output is the published command name.
#[test]
fn help_uses_published_binary_name() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("eval-magic"));
}

// Hidden guard entry points are implementation contracts rather than user journeys.
#[test]
fn every_visible_command_and_harness_subcommand_renders_help() {
    for args in [
        "run --help",
        "dispatch --help",
        "snapshot --help",
        "teardown --help",
        "teardown-guard --help",
        "ingest --help",
        "finalize --help",
        "record-runs --help",
        "fill-transcripts --help",
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
        skill_eval()
            .args(args.split_whitespace())
            .assert()
            .success()
            .stdout(contains("Usage:"));
    }
}

#[test]
fn init_help_documents_extended_eval_authoring() {
    skill_eval()
        .args(["init", "--help"])
        .assert()
        .success()
        .stdout(contains("turns"))
        .stdout(contains("files_root"))
        .stdout(contains("per-eval `runs`"))
        .stdout(contains("eval-magic validate"));
}

#[test]
fn run_help_documents_cost_and_runbook_authority() {
    skill_eval()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(contains("does not dispatch"))
        .stdout(contains("`2R` native agent sessions"))
        .stdout(contains("read `RUNBOOK.md` end to end"));
}

#[test]
fn top_level_examples_stop_after_orientation_and_handoffs() {
    skill_eval()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("eval-magic init"))
        .stdout(contains("eval-magic run"))
        .stdout(contains("RUNBOOK.md"))
        .stdout(contains("--agent-env TZ=America/Los_Angeles").not())
        .stdout(contains("harness show claude-code").not());
}

#[test]
fn dispatch_help_documents_conversation_verification() {
    skill_eval()
        .args(["dispatch", "--help"])
        .assert()
        .success()
        .stdout(contains("delivered_followups"))
        .stdout(contains("same native session ID"))
        .stdout(contains("interrupted"));
}

#[test]
fn aggregate_help_explains_how_to_read_results() {
    skill_eval()
        .args(["aggregate", "--help"])
        .assert()
        .success()
        .stdout(contains(
            "Read `validity_warnings` before trusting the delta",
        ))
        .stdout(contains("`n: 0` means unavailable"))
        .stdout(contains("smaller is not necessarily better"));
}

#[test]
fn promote_help_documents_baseline_artifacts() {
    skill_eval()
        .args(["promote-baseline", "--help"])
        .assert()
        .success()
        .stdout(contains("evals/baseline"))
        .stdout(contains("benchmark.json"))
        .stdout(contains("grading/"))
        .stdout(contains("NOTES.md"));
}

/// `--guard` and `--no-guard` are contradictory and rejected at parse time.
#[test]
fn run_rejects_guard_with_no_guard() {
    skill_eval()
        .args(["run", "--guard", "--no-guard"])
        .assert()
        .failure()
        .stderr(contains("cannot be used with"));
}

/// The auto-arm opt-out is a documented part of the `run` surface.
#[test]
fn run_help_documents_no_guard() {
    skill_eval()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(contains("--no-guard"));
}

#[test]
fn run_help_documents_task_local_scratch_policy() {
    skill_eval()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(contains("sole allowed write root"))
        .stdout(contains("<eval-root>/tmp"))
        .stdout(contains("host temp directories"));
}

#[test]
fn run_help_documents_task_git_repository_isolation() {
    skill_eval()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(contains("Git is required"))
        .stdout(contains("branch `work`"))
        .stdout(contains("no remotes"))
        .stdout(contains("Local Git operations"))
        .stdout(contains("remote Git operations"));
}

#[test]
fn grade_and_ingest_help_document_runner_owned_command_checks() {
    for command in ["grade", "ingest"] {
        skill_eval()
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(contains("command_check"))
            .stdout(contains("runner"))
            .stdout(contains("held-out"))
            .stdout(contains("environment matrix"));
    }
}

#[test]
fn dispatch_judges_help_documents_batch_completion() {
    // `dispatch --judges` owns the batch-completion contract: it is what counts
    // verdicts and decides the exit status.
    skill_eval()
        .args(["dispatch", "--help"])
        .assert()
        .success()
        .stdout(contains("Skips existing nonempty responses"))
        .stdout(contains("verdicts present"))
        .stdout(contains("exits nonzero while any are missing"));
}

#[test]
fn pipeline_help_documents_always_on_diff_scope_metrics() {
    for command in ["ingest", "grade", "finalize", "aggregate"] {
        skill_eval()
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(contains("diff_scope"))
            .stdout(contains("diff-scope.json"))
            .stdout(contains("files"))
            .stdout(contains("lines"));
    }
}

/// The patch is an artifact an operator has to be able to find, and the two
/// commands that produce it are the two that must name it.
#[test]
fn ingest_and_grade_help_document_the_captured_diff() {
    for command in ["ingest", "grade"] {
        skill_eval()
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(contains("diff.patch"));
    }
}

#[test]
fn finalize_and_aggregate_help_document_per_assertion_rollups() {
    for command in ["finalize", "aggregate"] {
        skill_eval()
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(contains("per-assertion"))
            .stdout(contains("passed"))
            .stdout(contains("observed assertion results"));
    }
}

#[test]
fn aggregate_help_documents_declared_shadow_isolation() {
    skill_eval()
        .args(["aggregate", "--help"])
        .assert()
        .success()
        .stdout(contains("plugin-shadow.json"))
        .stdout(contains("isolates_live_sources"))
        .stdout(contains("validity_warnings"))
        .stdout(contains("eval-magic docs isolation"));
}

#[test]
fn help_documents_guard_denial_artifacts_and_privacy() {
    skill_eval()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(contains("guard-denials.jsonl"))
        .stdout(contains("never the full command or patch"));

    for command in ["ingest", "detect-stray-writes", "aggregate"] {
        skill_eval()
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(contains("guard-denials.json"));
    }
}

/// `ingest` reaches its own context validation when invoked bare.
#[test]
fn ingest_is_wired_and_validates_context() {
    skill_eval()
        .arg("ingest")
        .assert()
        .failure()
        .stderr(contains("--skill-dir"));
}

/// `validate` over a dir of valid evals succeeds and prints a ✓ per file.
#[test]
fn validate_succeeds_on_valid_evals() {
    let tmp = TempDir::new().unwrap();
    write_evals(tmp.path(), "good", VALID_EVALS);

    skill_eval()
        .arg("validate")
        .arg("--skill-dir")
        .arg(tmp.path())
        .assert()
        .success()
        .stdout(contains("✓ good/evals/evals.json"))
        .stdout(contains("Validated 1 evals.json file(s); 0 failed."));
}

#[test]
fn validate_defaults_to_current_skill_dir() {
    let tmp = TempDir::new().unwrap();
    write_evals(tmp.path(), "good", VALID_EVALS);
    fs::write(
        tmp.path().join("good").join("SKILL.md"),
        "---\nname: good\n---\nbody\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(tmp.path().join("good"))
        .arg("validate")
        .assert()
        .success()
        .stdout(contains("✓ evals/evals.json"))
        .stdout(contains("Validated 1 evals.json file(s); 0 failed."));
}

#[test]
fn validate_accepts_a_skill_path() {
    let tmp = TempDir::new().unwrap();
    write_evals(tmp.path(), "good", VALID_EVALS);
    fs::write(
        tmp.path().join("good").join("SKILL.md"),
        "---\nname: good\n---\nbody\n",
    )
    .unwrap();

    skill_eval()
        .arg("validate")
        .arg("--skill")
        .arg(tmp.path().join("good"))
        .assert()
        .success()
        .stdout(contains("✓ evals/evals.json"));
}

/// `validate` exits non-zero and prints a ✗ when a file fails validation.
#[test]
fn validate_fails_on_invalid_evals() {
    let tmp = TempDir::new().unwrap();
    write_evals(tmp.path(), "bad", r#"{ "skill_name": "x", "evals": [] }"#);

    skill_eval()
        .arg("validate")
        .arg("--skill-dir")
        .arg(tmp.path())
        .assert()
        .failure()
        .stderr(contains("✗"));
}

/// `validate` without a detectable skill fails with our message.
#[test]
fn validate_requires_a_skill_context() {
    skill_eval()
        .arg("validate")
        .assert()
        .failure()
        .stderr(contains("missing skill"));
}

/// An unknown subcommand is rejected by the parser (clap), not silently
/// accepted.
#[test]
fn unknown_subcommand_is_rejected() {
    skill_eval().arg("does-not-exist").assert().failure();
}

/// An unknown `--harness` value is rejected by the registry resolver with an
/// error naming the offending value and every known harness. (Resolution
/// happens after parsing, not in clap, so runtime-loaded descriptors count.)
#[test]
fn unknown_harness_value_is_rejected_naming_known_harnesses() {
    skill_eval()
        .args(["aggregate", "--harness", "nonexistent"])
        .assert()
        .failure()
        .stderr(
            contains("unknown harness 'nonexistent'")
                .and(contains("claude-code"))
                .and(contains("codex"))
                .and(contains("opencode")),
        );
}

/// `run --help` names the built-in harnesses in the `--harness` doc text.
#[test]
fn run_help_names_builtin_harnesses() {
    skill_eval()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(
            contains("--harness")
                .and(contains("claude-code"))
                .and(contains("codex"))
                .and(contains("opencode")),
        );
}
