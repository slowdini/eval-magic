//! Harness-lint coverage for composable transcript capabilities.

use crate::helpers::skill_eval;
use predicates::str::contains;
use std::fs;
use tempfile::TempDir;

#[test]
fn harness_lint_accepts_a_surface_extract_overlaying_a_builtin_parser() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("claude-code.toml");
    fs::write(
        &file,
        "label = \"claude-code\"\n\n\
         [transcript]\n\
         events_filename = \"claude-events.jsonl\"\n\n\
         [transcript.extract.session_surface]\n\
         skills_field = \"skills\"\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .assert()
        .success()
        .stdout(contains("✓"));
}
