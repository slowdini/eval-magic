//! Portable tool-name matching for `tool_invocation_matches`: a pattern
//! authored against one harness's tool names grades the same on every other
//! harness, through the roles the descriptors declare.

use super::tests::{check, inv};
use super::*;
use crate::adapters::ToolVocabulary;
use crate::core::MustPrecede;
use serde_json::json;

/// Codex's `[tools]` vocabulary, as `harnesses/codex.toml` declares it.
fn codex() -> ToolVocabulary {
    ToolVocabulary {
        write_tools: vec!["Edit".into(), "Write".into(), "file_change".into()],
        patch_tools: vec!["apply_patch".into()],
        shell_tools: vec!["Bash".into(), "command_execution".into()],
        read_tools: vec![],
    }
}

/// OpenCode's vocabulary — note it declares no `Bash`, so its aliases can
/// only come from the registry-wide union.
fn opencode() -> ToolVocabulary {
    ToolVocabulary {
        write_tools: vec!["edit".into(), "write".into()],
        patch_tools: vec!["apply_patch".into()],
        shell_tools: vec!["bash".into()],
        read_tools: vec!["read".into(), "glob".into(), "grep".into()],
    }
}

/// The shape `all_tool_vocabulary()` builds: every descriptor's names,
/// unioned per role.
fn union() -> ToolVocabulary {
    ToolVocabulary {
        write_tools: vec![
            "Edit".into(),
            "MultiEdit".into(),
            "NotebookEdit".into(),
            "Write".into(),
            "edit".into(),
            "editor".into(),
            "file_change".into(),
            "write".into(),
        ],
        patch_tools: vec!["apply_patch".into()],
        shell_tools: vec![
            "Bash".into(),
            "bash".into(),
            "command_execution".into(),
            "run_commands".into(),
        ],
        read_tools: vec![
            "Glob".into(),
            "Grep".into(),
            "Read".into(),
            "glob".into(),
            "grep".into(),
            "read".into(),
            "read_files".into(),
            "search_codebase".into(),
        ],
    }
}

/// #308: a frozen `Bash|Read` pattern must grade a Codex `command_execution`
/// the same way it grades a Claude Code `Bash`.
#[test]
fn a_shell_role_alias_satisfies_a_foreign_native_name() {
    let (active, aliases) = (codex(), union());
    let invs = [inv("command_execution", json!({"command": "bun test"}), 0)];
    let r = grade_transcript_check_with_context(
        &check(Some("Bash|Read")),
        &invs,
        None,
        &ToolNaming::new(&active, &aliases),
    );
    assert!(r.passed, "{}", r.evidence);
    assert!(
        r.evidence.contains("via shell alias 'Bash'"),
        "evidence must name the alias that matched: {}",
        r.evidence
    );
    assert!(
        r.evidence
            .contains("command_execution {\"command\":\"bun test\"}"),
        "evidence must report the actual native invocation: {}",
        r.evidence
    );
}

/// The aliases come from the union, not the run's own descriptor: OpenCode
/// declares only `bash`, yet the same authored pattern still grades.
#[test]
fn aliases_come_from_the_union_not_only_the_active_descriptor() {
    let (active, aliases) = (opencode(), union());
    let invs = [inv("bash", json!({"command": "bun test"}), 0)];
    let r = grade_transcript_check_with_context(
        &check(Some("Bash|Read")),
        &invs,
        None,
        &ToolNaming::new(&active, &aliases),
    );
    assert!(r.passed, "{}", r.evidence);
    assert!(
        r.evidence.contains("via shell alias 'Bash'"),
        "{}",
        r.evidence
    );
}

/// A native-name pattern keeps its exact pre-alias evidence wording.
#[test]
fn a_native_match_wins_and_reports_no_alias() {
    let active = ToolVocabulary {
        shell_tools: vec!["Bash".into()],
        ..Default::default()
    };
    let aliases = union();
    let invs = [inv("Bash", json!({"command": "ls"}), 0)];
    let r = grade_transcript_check_with_context(
        &check(Some("Bash")),
        &invs,
        None,
        &ToolNaming::new(&active, &aliases),
    );
    assert!(r.passed, "{}", r.evidence);
    assert_eq!(r.evidence, "matched ordinal 0: Bash {\"command\":\"ls\"}");
}

/// A name the run's descriptor declares in no role gets no aliases.
#[test]
fn an_undeclared_tool_name_is_given_no_aliases() {
    let (active, aliases) = (codex(), union());
    let invs = [inv("web_search", json!({"query": "bun"}), 0)];
    let r = grade_transcript_check_with_context(
        &check(Some("Bash")),
        &invs,
        None,
        &ToolNaming::new(&active, &aliases),
    );
    assert!(!r.passed, "{}", r.evidence);
}

/// Aliasing is role-scoped: a write-role invocation is never rewritten with a
/// shell-role name.
#[test]
fn aliases_do_not_cross_roles() {
    let (active, aliases) = (codex(), union());
    let invs = [inv("file_change", json!({"path": "src/x.rs"}), 0)];
    let shell = grade_transcript_check_with_context(
        &check(Some("Bash")),
        &invs,
        None,
        &ToolNaming::new(&active, &aliases),
    );
    assert!(!shell.passed, "{}", shell.evidence);
    let write = grade_transcript_check_with_context(
        &check(Some("MultiEdit")),
        &invs,
        None,
        &ToolNaming::new(&active, &aliases),
    );
    assert!(write.passed, "{}", write.evidence);
    assert!(
        write.evidence.contains("via write alias 'MultiEdit'"),
        "{}",
        write.evidence
    );
}

/// Arguments survive the name substitution, so a pattern spanning the
/// name/args boundary still grades.
#[test]
fn an_argument_regex_matches_across_an_alias_variant() {
    let (active, aliases) = (codex(), union());
    let invs = [inv(
        "command_execution",
        json!({"command": "bash -lc 'bun test'"}),
        0,
    )];
    let r = grade_transcript_check_with_context(
        &check(Some("Bash.*bun test")),
        &invs,
        None,
        &ToolNaming::new(&active, &aliases),
    );
    assert!(r.passed, "{}", r.evidence);
}

/// An alias-only match still honors `must_precede`.
#[test]
fn an_alias_match_before_the_first_write_satisfies_the_ordering_constraint() {
    let (active, aliases) = (codex(), union());
    let mut assertion = check(Some("Bash"));
    assertion.must_precede = Some(MustPrecede::FirstWrite);
    let invs = [
        inv("command_execution", json!({"command": "bun test"}), 0),
        inv("file_change", json!({"path": "src/x.rs"}), 1),
    ];
    let r = grade_transcript_check_with_context(
        &assertion,
        &invs,
        None,
        &ToolNaming::new(&active, &aliases),
    );
    assert!(r.passed, "{}", r.evidence);
    assert!(r.evidence.contains("ordinal 0"), "{}", r.evidence);
}

#[test]
fn an_alias_match_after_the_first_write_fails_the_ordering_constraint() {
    let (active, aliases) = (codex(), union());
    let mut assertion = check(Some("Bash"));
    assertion.must_precede = Some(MustPrecede::FirstWrite);
    let invs = [
        inv("file_change", json!({"path": "src/x.rs"}), 0),
        inv("command_execution", json!({"command": "bun test"}), 1),
    ];
    let r = grade_transcript_check_with_context(
        &assertion,
        &invs,
        None,
        &ToolNaming::new(&active, &aliases),
    );
    assert!(!r.passed);
    assert!(
        r.evidence.contains("1 match(es)") && r.evidence.contains("first write"),
        "{}",
        r.evidence
    );
}

/// No harness in `conditions.json` (a legacy iteration) means no roles and no
/// aliases — grading falls back to exactly the pre-#308 behavior.
#[test]
fn an_empty_vocabulary_grades_native_names_only() {
    let active = ToolVocabulary::default();
    let invs = [inv("command_execution", json!({"command": "ls"}), 0)];
    let r = grade_transcript_check_with_context(
        &check(Some("Bash")),
        &invs,
        None,
        &ToolNaming::without_aliases(&active),
    );
    assert!(!r.passed);
    assert_eq!(
        r.evidence,
        "no candidate matched /Bash/ across 1 invocation(s)"
    );
}

/// A miss names the roles that were expanded, so an operator can see that
/// alias matching was applied and still found nothing.
#[test]
fn a_miss_names_the_roles_whose_aliases_were_tried() {
    let (active, aliases) = (codex(), union());
    let invs = [
        inv("command_execution", json!({"command": "ls"}), 0),
        inv("file_change", json!({"path": "src/x.rs"}), 1),
    ];
    let r = grade_transcript_check_with_context(
        &check(Some("NoSuchTool")),
        &invs,
        None,
        &ToolNaming::new(&active, &aliases),
    );
    assert!(!r.passed);
    assert_eq!(
        r.evidence,
        "no candidate matched /NoSuchTool/ across 2 invocation(s) \
         (native names plus write/shell role aliases)"
    );
}
