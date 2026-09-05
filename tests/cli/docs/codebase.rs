//! Contract checks for the shipped codebase guide.

use crate::helpers::skill_eval;
use predicates::str::contains;

/// The parts a config author cannot infer have to survive an edit: init's
/// source modes and project choices, the required git ref, overlay semantics,
/// local-path portability, cache provisioning, and the measured baseline.
#[test]
fn keeps_declaration_rules_project_choices_caveat_and_provisioning_contract() {
    skill_eval()
        .args(["docs", "codebase"])
        .assert()
        .success()
        .stdout(contains("# Sourcing a codebase into a task environment"))
        .stdout(contains("eval-magic init"))
        .stdout(contains("--codebase-url"))
        .stdout(contains("--codebase-ref"))
        .stdout(contains("--codebase-path"))
        .stdout(contains("--codebase-cwd"))
        .stdout(contains("Weeknight"))
        .stdout(contains("eval-magic as a complex project"))
        .stdout(contains("\"ref\""))
        .stdout(contains("`ref` is required"))
        .stdout(contains("overlay"))
        .stdout(contains("refs/eval-magic/baseline"))
        .stdout(contains("host_local"))
        .stdout(contains("not reproducible"))
        .stdout(contains("materialized once"))
        .stdout(contains("hard-link"))
        .stdout(contains("independent working tree"))
        .stdout(contains("diff-scope.json"))
        .stdout(contains("diff.patch"))
        .stdout(contains(".gitignore"))
        .stdout(contains("exclude_skill_sources"))
        .stdout(contains("codebase-sourced"))
        .stdout(contains("CLAUDE.md"))
        .stdout(contains(".opencode/skills"));
}

/// A codebase's own linters must not be able to see the framework's staged
/// files, and an author has to be able to find both the detected set and the
/// override without reading the source.
#[test]
fn keeps_the_framework_ignore_contract_and_its_override() {
    skill_eval()
        .args(["docs", "codebase"])
        .assert()
        .success()
        .stdout(contains("ignore_files"))
        .stdout(contains(".prettierignore"))
        .stdout(contains(".eslintignore"))
        .stdout(contains(">>> eval-magic framework files >>>"))
        .stdout(contains("both arms"))
        .stdout(contains("replaces detection"))
        .stdout(contains("`.gitignore` is never a target"))
        .stdout(contains("refs/eval-magic/baseline:.prettierignore"));
}
