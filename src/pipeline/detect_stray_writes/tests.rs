//! Unit tests for [`super`]: stray-write classification, live-source-read
//! detection, and the task boundaries both are measured against.
//!
//! A sibling rather than an inline module, because the two together outgrow one
//! file.

use super::*;
use serde_json::json;

const ALLOWED_ROOT: &str = "/work/iteration-1/env-g1-with_skill";
const REPO: &str = "/work/repo";
const LIVE_SKILL: &str = "/work/repo/skills/mr-review";

/// Build a minimal invocation from name/args/ordinal (result is unused here).
fn inv(name: &str, args: serde_json::Value, ordinal: u32) -> ToolInvocation {
    ToolInvocation {
        name: name.to_string(),
        args: Some(args),
        result: None,
        ordinal,
    }
}

fn repo() -> &'static Path {
    Path::new(REPO)
}

fn live() -> &'static Path {
    Path::new(LIVE_SKILL)
}

// --- detectStrayWrites ---

#[test]
fn a_write_inside_the_task_environment_is_clean() {
    let f = detect_stray_writes(
        &[inv(
            "Write",
            json!({"file_path": format!("{ALLOWED_ROOT}/answer.md")}),
            0,
        )],
        ALLOWED_ROOT,
        repo(),
    );
    assert!(f.violations.is_empty());
    assert!(f.warnings.is_empty());
}

#[test]
fn a_relative_write_resolves_from_the_task_environment() {
    let f = detect_stray_writes(
        &[inv("Edit", json!({"file_path": "src/lib.rs"}), 0)],
        ALLOWED_ROOT,
        Path::new(ALLOWED_ROOT),
    );
    assert!(f.violations.is_empty());
}

#[test]
fn a_write_outside_the_task_environment_is_a_violation() {
    let f = detect_stray_writes(
        &[inv(
            "Write",
            json!({"file_path": format!("{REPO}/runner/run.ts")}),
            2,
        )],
        ALLOWED_ROOT,
        repo(),
    );
    assert_eq!(f.violations.len(), 1);
    assert_eq!(f.violations[0].tool, "Write");
    assert_eq!(
        f.violations[0].path.as_deref(),
        Some(&*format!("{REPO}/runner/run.ts"))
    );
    assert_eq!(f.violations[0].ordinal, 2);
}

#[test]
fn edit_multiedit_notebookedit_outside_the_task_environment_is_a_violation() {
    let f = detect_stray_writes(
        &[
            inv("Edit", json!({"file_path": "/etc/hosts"}), 0),
            inv("NotebookEdit", json!({"notebook_path": "/tmp/x.ipynb"}), 1),
        ],
        ALLOWED_ROOT,
        repo(),
    );
    let mut tools: Vec<&str> = f.violations.iter().map(|v| v.tool.as_str()).collect();
    tools.sort();
    assert_eq!(tools, vec!["Edit", "NotebookEdit"]);
}

#[test]
fn an_install_command_is_a_warning() {
    let f = detect_stray_writes(
        &[inv("Bash", json!({"command": "npm install left-pad"}), 0)],
        ALLOWED_ROOT,
        repo(),
    );
    assert_eq!(f.warnings.len(), 1);
    assert_eq!(f.warnings[0].tool, "Bash");
    assert!(f.warnings[0].reason.to_lowercase().contains("install"));
}

#[test]
fn configured_command_policy_is_shared_with_the_stray_write_audit() {
    let policy = crate::core::GuardPolicyConfig {
        allow_commands: vec!["cargo test".to_string()],
        ..crate::core::GuardPolicyConfig::default()
    };

    let findings = detect_stray_writes_with_policy(
        &[inv("Bash", json!({"command": "cargo test --workspace"}), 0)],
        ALLOWED_ROOT,
        Path::new(ALLOWED_ROOT),
        &policy,
    );

    assert!(findings.warnings.is_empty());
}

#[test]
fn a_codex_command_execution_install_is_a_warning() {
    let f = detect_stray_writes(
        &[inv(
            "command_execution",
            json!({"command": "npm install left-pad"}),
            0,
        )],
        ALLOWED_ROOT,
        repo(),
    );
    assert_eq!(f.warnings.len(), 1);
    assert_eq!(f.warnings[0].tool, "command_execution");
    assert!(f.warnings[0].reason.to_lowercase().contains("install"));
}

#[test]
fn a_codex_file_change_outside_the_task_environment_is_a_violation() {
    let f = detect_stray_writes(
        &[inv(
            "file_change",
            json!({"path": format!("{REPO}/src/app.ts")}),
            4,
        )],
        ALLOWED_ROOT,
        repo(),
    );
    assert_eq!(f.violations.len(), 1);
    assert_eq!(f.violations[0].tool, "file_change");
    assert_eq!(
        f.violations[0].path.as_deref(),
        Some(&*format!("{REPO}/src/app.ts"))
    );
    assert_eq!(f.violations[0].ordinal, 4);
}

#[test]
fn a_mutating_bash_scoped_to_the_task_environment_is_not_flagged() {
    let f = detect_stray_writes(
        &[inv(
            "Bash",
            json!({"command": format!("echo hi > {ALLOWED_ROOT}/log.txt")}),
            0,
        )],
        ALLOWED_ROOT,
        repo(),
    );
    assert!(f.warnings.is_empty());
}

#[test]
fn a_relative_redirection_resolves_from_the_task_environment() {
    let f = detect_stray_writes(
        &[inv("Bash", json!({"command": "printf done > notes.md"}), 0)],
        ALLOWED_ROOT,
        Path::new(ALLOWED_ROOT),
    );
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
}

#[test]
fn git_worktree_add_is_a_warning() {
    let f = detect_stray_writes(
        &[inv(
            "Bash",
            json!({"command": "git worktree add ../wt -b scratch"}),
            0,
        )],
        ALLOWED_ROOT,
        repo(),
    );
    assert_eq!(f.warnings.len(), 1);
    assert!(f.warnings[0].reason.to_lowercase().contains("worktree"));
}

#[test]
fn read_only_tools_are_never_flagged() {
    let f = detect_stray_writes(
        &[
            inv("Read", json!({"file_path": "/anywhere"}), 0),
            inv("Grep", json!({"pattern": "x"}), 1),
            inv("Bash", json!({"command": "ls -la /"}), 2),
        ],
        ALLOWED_ROOT,
        repo(),
    );
    assert!(f.violations.is_empty());
    assert!(f.warnings.is_empty());
}

// --- detectLiveSourceReads ---

#[test]
fn a_read_of_the_live_skill_md_is_flagged() {
    let f = detect_live_source_reads(
        &[inv(
            "Read",
            json!({"file_path": format!("{LIVE_SKILL}/SKILL.md")}),
            1,
        )],
        live(),
        repo(),
    );
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].tool, "Read");
    assert_eq!(
        f[0].path.as_deref(),
        Some(&*format!("{LIVE_SKILL}/SKILL.md"))
    );
    assert_eq!(f[0].ordinal, 1);
    assert!(f[0].reason.to_lowercase().contains("live skill source"));
}

#[test]
fn a_read_of_a_staged_eval_copy_is_not_flagged() {
    let f = detect_live_source_reads(
        &[inv(
            "Read",
            json!({"file_path": format!("{REPO}/.claude/skills/slow-powers-eval-1-old_skill__mr-review/SKILL.md")}),
            0,
        )],
        live(),
        repo(),
    );
    assert!(f.is_empty());
}

#[test]
fn a_relative_read_resolving_under_the_live_dir_is_flagged() {
    let f = detect_live_source_reads(
        &[inv(
            "Read",
            json!({"file_path": "skills/mr-review/SKILL.md"}),
            0,
        )],
        live(),
        repo(),
    );
    assert_eq!(f.len(), 1);
}

#[test]
fn a_grep_scoped_to_the_live_dir_is_flagged() {
    let f = detect_live_source_reads(
        &[inv("Grep", json!({"pattern": "x", "path": LIVE_SKILL}), 2)],
        live(),
        repo(),
    );
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].tool, "Grep");
}

#[test]
fn a_bash_referencing_the_live_dir_absolutely_is_flagged() {
    let f = detect_live_source_reads(
        &[inv(
            "Bash",
            json!({"command": format!("grep -r trigger {LIVE_SKILL}/")}),
            0,
        )],
        live(),
        repo(),
    );
    assert_eq!(f.len(), 1);
}

/// A contaminated arm is not comparable data, so the scan has to hold when
/// the command spells the live directory with the other separator — which
/// on Windows is every command, since the recorded directory is a host path.
#[test]
fn a_bash_spelling_the_live_dir_with_the_other_separator_is_flagged() {
    let f = detect_live_source_reads(
        &[inv(
            "Bash",
            json!({"command": r"cat \work\repo\skills\mr-review\SKILL.md"}),
            0,
        )],
        live(),
        repo(),
    );
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].tool, "Bash");
}

#[test]
fn unrelated_reads_and_commands_are_not_flagged() {
    let f = detect_live_source_reads(
        &[
            inv(
                "Read",
                json!({"file_path": format!("{ALLOWED_ROOT}/x.md")}),
                0,
            ),
            inv("Bash", json!({"command": "ls .eval-magic"}), 1),
            // Write tools are detect_stray_writes' jurisdiction — reads only here.
            inv(
                "Write",
                json!({"file_path": format!("{LIVE_SKILL}/SKILL.md")}),
                2,
            ),
        ],
        live(),
        repo(),
    );
    assert!(f.is_empty());
}

// --- live-source reads through an alias ---

/// One live skill directory reached two ways: `real/skills/mr-review`, and
/// `alias/skills/mr-review` where `alias` is a symlink to `real`. Returns
/// the resolved directory the runner would record and the alias spelling an
/// agent could type. `None` when this filesystem forbids links.
fn aliased_live_skill(tmp: &Path, test: &str) -> Option<(PathBuf, PathBuf)> {
    if crate::core::fs::skip_without_symlinks(tmp, test) {
        return None;
    }
    let real = tmp.join("real");
    let skill = real.join("skills/mr-review");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "# mr-review").unwrap();
    let alias = tmp.join("alias");
    crate::core::fs::create_symlink(&real, &alias).unwrap();
    Some((skill, alias.join("skills/mr-review")))
}

/// The runner records the live directory resolved, while the agent records
/// whatever it typed. Two spellings of one file are one file, so the read is
/// contamination either way.
#[test]
fn a_read_through_an_alias_of_the_live_dir_is_flagged() {
    let tmp = tempfile::TempDir::new().unwrap();
    let Some((live, alias)) = aliased_live_skill(
        tmp.path(),
        "a_read_through_an_alias_of_the_live_dir_is_flagged",
    ) else {
        return;
    };
    let read = alias.join("SKILL.md");

    let f = detect_live_source_reads(
        &[inv("Read", json!({"file_path": read.to_string_lossy()}), 0)],
        &live,
        tmp.path(),
    );

    assert_eq!(f.len(), 1, "{f:?}");
    // The evidence is what the agent actually typed, not its resolution.
    assert_eq!(f[0].path.as_deref(), Some(&*read.to_string_lossy()));
}

#[test]
fn a_bash_referencing_the_live_dir_through_an_alias_is_flagged() {
    let tmp = tempfile::TempDir::new().unwrap();
    let Some((live, alias)) = aliased_live_skill(
        tmp.path(),
        "a_bash_referencing_the_live_dir_through_an_alias_is_flagged",
    ) else {
        return;
    };
    let command = format!("cat {}/SKILL.md", alias.display());

    let f = detect_live_source_reads(
        &[inv("Bash", json!({"command": command.clone()}), 0)],
        &live,
        tmp.path(),
    );

    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].command.as_deref(), Some(&*command));
}

/// Resolution must not widen the boundary: an alias is only a live-source
/// read when it lands inside the live directory.
#[test]
fn an_alias_of_an_unrelated_directory_is_not_flagged() {
    let tmp = tempfile::TempDir::new().unwrap();
    let Some((live, _)) = aliased_live_skill(
        tmp.path(),
        "an_alias_of_an_unrelated_directory_is_not_flagged",
    ) else {
        return;
    };
    let elsewhere = tmp.path().join("real/docs");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let alias = tmp.path().join("docs-alias");
    crate::core::fs::create_symlink(&elsewhere, &alias).unwrap();
    let read = alias.join("guide.md");

    let f = detect_live_source_reads(
        &[
            inv("Read", json!({"file_path": read.to_string_lossy()}), 0),
            inv(
                "Bash",
                json!({"command": format!("cat {}", read.display())}),
                1,
            ),
        ],
        &live,
        tmp.path(),
    );

    assert!(f.is_empty(), "{f:?}");
}

/// A shell word is only a live-source reference when it names a path. A
/// bare command name resolves against the runner's cwd like any relative
/// word, so without this every command would be a finding whenever that cwd
/// sits inside the live directory.
#[test]
fn a_bare_command_word_is_not_a_path_into_the_live_dir() {
    let tmp = tempfile::TempDir::new().unwrap();
    let live = tmp.path().join("skills/mr-review");
    std::fs::create_dir_all(&live).unwrap();

    let f = detect_live_source_reads(
        &[inv("Bash", json!({"command": "cargo test"}), 0)],
        &live,
        &live,
    );

    assert!(f.is_empty(), "{f:?}");
}

// --- declared plan-file root ---

/// A harness that writes its plan to a file outside the env (Claude Code's
/// `~/.claude/plans`) declares that root; writes there are the plan, not a
/// stray write.
#[test]
fn a_write_under_a_declared_plan_file_root_is_clean() {
    let roots = [
        ALLOWED_ROOT.to_string(),
        "/Users/someone/.claude/plans".to_string(),
    ];
    let f = detect_stray_writes_in(
        &[
            inv(
                "Write",
                json!({"file_path": "/Users/someone/.claude/plans/fix.md"}),
                0,
            ),
            inv(
                "Write",
                json!({"file_path": "/Users/someone/.claude/other.md"}),
                1,
            ),
        ],
        &roots,
        repo(),
        &GuardPolicyConfig::default(),
    );
    assert_eq!(f.violations.len(), 1, "{:?}", f.violations);
    assert_eq!(
        f.violations[0].path.as_deref(),
        Some("/Users/someone/.claude/other.md")
    );
}

#[test]
fn task_boundaries_carry_the_frozen_plan_file_root() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("dispatch.json"),
        serde_json::to_string(&json!({
            "harness_descriptor": {
                "label": "claude-code",
                "plan_mode": {
                    "plan_args": " --permission-mode plan",
                    "act_args": " --permission-mode bypassPermissions",
                    "plan_file": { "root": "~/.claude/plans", "content_field": "content" }
                }
            },
            "tasks": [
                { "eval_id": "e1", "condition": "with_skill", "eval_root": ALLOWED_ROOT }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let home = Path::new("/Users/someone");
    let boundaries = task_boundaries_by_key_with_home(tmp.path(), Some(home));
    let boundary = boundaries.get("e1:with_skill").expect("task boundary");
    assert_eq!(
        boundary.allowed_roots,
        vec![
            ALLOWED_ROOT.to_string(),
            "/Users/someone/.claude/plans".to_string()
        ]
    );
}
