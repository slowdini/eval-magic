//! `init` subcommand: scaffold a first evals/evals.json for a skill.

use crate::helpers::{canonical_root, skill_eval};
use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_CODEBASE_URL: &str = "https://github.com/slowdini/eval-magic-fixture";
const DEFAULT_CODEBASE_REF: &str = "b6d269c1cdedf7cadb53bacc41acaf5f2cdbe03f";

/// Write `<root>/skill-dir/mr-review/SKILL.md` and return `(skill_dir, skill_sub)`.
fn write_skill(root: &Path) -> (PathBuf, PathBuf) {
    let skill_dir = root.join("skill-dir");
    let skill_sub = skill_dir.join("mr-review");
    fs::create_dir_all(&skill_sub).unwrap();
    fs::write(
        skill_sub.join("SKILL.md"),
        "---\nname: mr-review\ndescription: review MRs\n---\n\nbody\n",
    )
    .unwrap();
    (skill_dir, skill_sub)
}

fn seeded_init(skill_dir: &Path) -> Command {
    let mut command = skill_eval();
    command.args(["init", "--skill-dir"]).arg(skill_dir).args([
        "--skill",
        "mr-review",
        "--id",
        "claim-without-running",
        "--prompt",
        "hey can you check the tests pass",
        "--expected-output",
        "Runs the test command and quotes real output",
    ]);
    command
}

#[test]
fn init_with_flags_writes_valid_seed_evals() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill(&root);

    skill_eval()
        .args(["init", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--id",
            "claim-without-running",
            "--prompt",
            "hey can you check the tests pass",
            "--expected-output",
            "Runs the test command and quotes real output",
        ])
        .assert()
        .success()
        .stderr("")
        .stdout(contains("Initialized evals for mr-review"))
        .stdout(contains("eval-magic run --skill-dir"))
        .stdout(contains("follow the generated RUNBOOK.md"))
        .stdout(contains("eval-magic ingest").not())
        .stdout(contains("eval-magic finalize").not())
        .stdout(contains("eval-magic promote-baseline").not());

    let written = fs::read_to_string(skill_sub.join("evals/evals.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(
        parsed,
        json!({
            "skill_name": "mr-review",
            "codebase": {
                "url": DEFAULT_CODEBASE_URL,
                "ref": DEFAULT_CODEBASE_REF
            },
            "evals": [
                {
                    "id": "claim-without-running",
                    "prompt": "hey can you check the tests pass",
                    "expected_output": "Runs the test command and quotes real output"
                }
            ]
        })
    );
}

#[test]
fn init_default_codebase_is_pinned_in_the_shipped_guide() {
    skill_eval()
        .args(["docs", "codebase"])
        .assert()
        .success()
        .stdout(contains(DEFAULT_CODEBASE_URL))
        .stdout(contains(DEFAULT_CODEBASE_REF));
}

#[test]
fn init_writes_an_explicit_url_and_ref_without_contacting_the_remote() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill(&root);

    seeded_init(&skill_dir)
        .args([
            "--codebase-url",
            "https://invalid.invalid/not-a-repository",
            "--codebase-ref",
            "0123456789abcdef0123456789abcdef01234567",
        ])
        .assert()
        .success();

    let written = fs::read_to_string(skill_sub.join("evals/evals.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(
        parsed["codebase"],
        json!({
            "url": "https://invalid.invalid/not-a-repository",
            "ref": "0123456789abcdef0123456789abcdef01234567"
        })
    );
}

#[test]
fn init_requires_url_and_ref_together() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, _skill_sub) = write_skill(&root);

    seeded_init(&skill_dir)
        .args(["--codebase-url", "https://example.com/repository"])
        .assert()
        .failure()
        .stderr(contains("--codebase-ref"));

    seeded_init(&skill_dir)
        .args(["--codebase-ref", "main"])
        .assert()
        .failure()
        .stderr(contains("--codebase-url"));
}

#[test]
fn init_rejects_multiple_codebase_source_modes() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, _skill_sub) = write_skill(&root);

    seeded_init(&skill_dir)
        .args([
            "--codebase-url",
            "https://example.com/repository",
            "--codebase-ref",
            "main",
            "--codebase-path",
            ".",
        ])
        .assert()
        .failure()
        .stderr(contains("cannot be used with"));

    seeded_init(&skill_dir)
        .args(["--codebase-path", ".", "--codebase-cwd"])
        .assert()
        .failure()
        .stderr(contains("cannot be used with"));
}

#[test]
fn init_resolves_a_relative_codebase_path_from_the_invocation_cwd() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill(&root);
    fs::create_dir(root.join("source")).unwrap();

    seeded_init(&skill_dir)
        .current_dir(&root)
        .args(["--codebase-path", "source"])
        .assert()
        .success();

    let written = fs::read_to_string(skill_sub.join("evals/evals.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(parsed["codebase"], json!({ "path": "../../../source" }));
}

#[test]
fn init_can_use_the_invocation_cwd_as_the_codebase() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill(&root);
    let codebase = root.join("source");
    fs::create_dir(&codebase).unwrap();

    seeded_init(&skill_dir)
        .current_dir(&codebase)
        .arg("--codebase-cwd")
        .assert()
        .success();

    let written = fs::read_to_string(skill_sub.join("evals/evals.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(parsed["codebase"], json!({ "path": "../../../source" }));
}

#[test]
fn init_preserves_an_absolute_codebase_path_after_canonicalizing_it() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill(&root);
    let codebase = root.join("source");
    fs::create_dir(&codebase).unwrap();

    seeded_init(&skill_dir)
        .args(["--codebase-path"])
        .arg(&codebase)
        .assert()
        .success();

    let written = fs::read_to_string(skill_sub.join("evals/evals.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(parsed["codebase"], json!({ "path": codebase }));
}

#[test]
fn init_rejects_a_missing_or_non_directory_codebase_before_prompting() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, _skill_sub) = write_skill(&root);
    let file = root.join("not-a-directory");
    fs::write(&file, "content").unwrap();

    skill_eval()
        .args(["init", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--codebase-path", "missing"])
        .current_dir(&root)
        .assert()
        .failure()
        .stdout("")
        .stderr(contains("--codebase-path is not a directory"));

    skill_eval()
        .args(["init", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--codebase-path"])
        .arg(&file)
        .assert()
        .failure()
        .stdout("")
        .stderr(contains("--codebase-path is not a directory"));
}

/// Even when `init` runs from inside the skill dir, the printed "Next:" commands
/// must be copy-pasteable: each carries `--skill-dir`/`--skill` so it resolves
/// from any cwd.
#[test]
fn init_from_skill_dir_prints_copy_pasteable_next_steps() {
    let (_tmp, root) = canonical_root();
    let (_skill_dir, skill_sub) = write_skill(&root);

    skill_eval()
        .current_dir(&skill_sub)
        .args([
            "init",
            "--id",
            "claim-without-running",
            "--prompt",
            "hey can you check the tests pass",
            "--expected-output",
            "Runs the test command and quotes real output",
        ])
        .assert()
        .success()
        .stdout(contains("  eval-magic run --skill-dir"))
        .stdout(contains("--skill mr-review --workspace-dir"))
        .stdout(contains("--guard").not())
        .stdout(contains("follow the generated RUNBOOK.md"))
        .stdout(contains("  eval-magic ingest --skill-dir").not())
        .stdout(contains("  eval-magic finalize --skill-dir").not())
        .stdout(contains("  eval-magic promote-baseline --skill-dir").not());
}

#[test]
fn init_prompts_for_missing_seed_fields() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill(&root);

    skill_eval()
        .args(["init", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .write_stdin(
            "claim-without-running\nhey can you check the tests pass\nRuns the test command and quotes real output\n",
        )
        .assert()
        .success()
        .stderr("")
        .stdout(contains("Eval id"))
        .stdout(contains("Prompt"))
        .stdout(contains("Expected output"));

    let written = fs::read_to_string(skill_sub.join("evals/evals.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(parsed["skill_name"], "mr-review");
    assert_eq!(parsed["evals"][0]["id"], "claim-without-running");
    assert_eq!(
        parsed["evals"][0]["expected_output"],
        "Runs the test command and quotes real output"
    );
}

#[test]
fn init_refuses_to_overwrite_existing_evals_without_force() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill(&root);
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    fs::write(skill_sub.join("evals/evals.json"), r#"{"already":true}"#).unwrap();

    skill_eval()
        .args(["init", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--id",
            "new-case",
            "--prompt",
            "new prompt",
            "--expected-output",
            "new output",
        ])
        .assert()
        .failure()
        .stderr(contains("evals.json already exists"))
        .stderr(contains("--force"));

    assert_eq!(
        fs::read_to_string(skill_sub.join("evals/evals.json")).unwrap(),
        r#"{"already":true}"#
    );
}

#[test]
fn init_refuses_existing_evals_before_prompting_for_seed_fields() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill(&root);
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    fs::write(skill_sub.join("evals/evals.json"), r#"{"already":true}"#).unwrap();

    skill_eval()
        .args(["init", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review"])
        .assert()
        .failure()
        .stdout("")
        .stderr(contains("evals.json already exists"));
}

#[test]
fn init_force_overwrites_existing_evals() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill(&root);
    fs::create_dir_all(skill_sub.join("evals")).unwrap();
    fs::write(skill_sub.join("evals/evals.json"), r#"{"already":true}"#).unwrap();

    skill_eval()
        .args(["init", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--id",
            "new-case",
            "--prompt",
            "new prompt",
            "--expected-output",
            "new output",
            "--force",
        ])
        .assert()
        .success()
        .stdout(contains("Initialized evals for mr-review"));

    let written = fs::read_to_string(skill_sub.join("evals/evals.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(parsed["evals"][0]["id"], "new-case");
}

#[test]
fn init_writes_negative_eval_marker_when_skill_should_not_trigger() {
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_sub) = write_skill(&root);

    skill_eval()
        .args(["init", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--id",
            "unrelated-request",
            "--prompt",
            "add a verbose flag",
            "--expected-output",
            "Does not invoke the review skill",
            "--skill-should-trigger",
            "false",
        ])
        .assert()
        .success();

    let written = fs::read_to_string(skill_sub.join("evals/evals.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(parsed["evals"][0]["skill_should_trigger"], false);
}

#[test]
fn init_requires_an_existing_skill_md() {
    let (_tmp, root) = canonical_root();
    let skill_dir = root.join("skill-dir");
    fs::create_dir_all(skill_dir.join("mr-review")).unwrap();

    skill_eval()
        .args(["init", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--id",
            "new-case",
            "--prompt",
            "new prompt",
            "--expected-output",
            "new output",
        ])
        .assert()
        .failure()
        .stderr(contains("skill not found"))
        .stderr(contains("SKILL.md"));
}

#[test]
fn init_help_documents_the_full_scaffold_workflow() {
    skill_eval()
        .args(["init", "--help"])
        .assert()
        .success()
        .stdout(contains(
            "does not run agents, ingest transcripts, finalize, or promote",
        ))
        .stdout(contains("If omitted, prompts interactively"))
        .stdout(contains(
            "Defaults to true and is omitted from the generated JSON",
        ))
        .stdout(contains("Set false for negative evals"))
        .stdout(contains("default example codebase"))
        .stdout(contains("--codebase-cwd"))
        .stdout(contains("--codebase-url"))
        .stdout(contains("--codebase-ref"))
        .stdout(contains("--codebase-path"))
        .stdout(contains("Refuses to overwrite existing evals by default"));
}
