//! Bounded, reusable evidence rendered once for every recorded run.

mod bounds;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::fs::artifact_path;
use crate::core::{ConversationEvent, RunRecord, ToolInvocation};
use crate::pipeline::diff_scope::{ChangedFile, DiffScopeMetrics, PatchRecord};
use crate::pipeline::error::PipelineError;

use bounds::{Rendered, bounded_excerpt, bounded_fenced};

/// Maximum size of the persisted evidence bundle embedded in judge prompts.
pub const EVIDENCE_BUNDLE_BYTE_LIMIT: usize = 98_304;
/// Maximum size of one complete judge prompt, including its rubric.
pub const JUDGE_PROMPT_BYTE_LIMIT: usize = 131_072;

const TASK_PROMPT_BYTE_LIMIT: usize = 8 * 1024;
const FINAL_MESSAGE_BYTE_LIMIT: usize = 12 * 1024;
/// The approved plan of a plan-mode run, read from the driver's `plan.md`.
const PLAN_BYTE_LIMIT: usize = 8 * 1024;
const CHANGED_FILES_BYTE_LIMIT: usize = 8 * 1024;
const CONVERSATION_BYTE_LIMIT: usize = 16 * 1024;
const CONVERSATION_EVENT_BYTE_LIMIT: usize = 4 * 1024;
const TOOL_SUMMARY_BYTE_LIMIT: usize = 8 * 1024;
const TOOL_FIELD_BYTE_LIMIT: usize = 512;

/// The public pointer and accounting carried by every emitted judge task.
#[derive(Debug, Clone, Serialize)]
pub struct EvidenceBundleRef {
    pub path: String,
    pub bytes: usize,
    pub byte_limit: usize,
    pub truncated: bool,
}

/// One persisted bundle and the metadata serialized into `judge-tasks.json`.
pub struct EvidenceBundle {
    pub content: String,
    pub reference: EvidenceBundleRef,
}

#[derive(Debug, Deserialize)]
struct CapturedDiff {
    #[serde(flatten)]
    metrics: DiffScopeMetrics,
    #[serde(default)]
    files: Vec<ChangedFile>,
    #[serde(default)]
    patch: Option<PatchRecord>,
}

struct DiffEvidence {
    summary: Rendered,
    patch: String,
    source_truncated: bool,
}

/// Render the bounded evidence persisted for one recorded run.
pub fn build_evidence_bundle(
    run_record: &RunRecord,
    run_record_path: &Path,
    outputs_dir: &Path,
    bundle_path: &Path,
) -> Result<EvidenceBundle, PipelineError> {
    let run_dir = run_record_path.parent().ok_or_else(|| {
        PipelineError::Message(format!(
            "judge evidence run record has no parent directory: {}",
            run_record_path.display()
        ))
    })?;
    let diff_scope_path = run_dir.join("diff-scope.json");
    let captured_diff: Option<CapturedDiff> = if diff_scope_path.exists() {
        Some(serde_json::from_str(&fs::read_to_string(
            &diff_scope_path,
        )?)?)
    } else {
        None
    };
    let patch_path = captured_diff
        .as_ref()
        .and_then(|diff| diff.patch.as_ref())
        .map(|patch| run_dir.join(&patch.path))
        .unwrap_or_else(|| run_dir.join("diff.patch"));

    let accounting_reserve = evidence_accounting(EVIDENCE_BUNDLE_BYTE_LIMIT, false);
    let mut sections = vec![
        "# Judge evidence bundle".to_string(),
        String::new(),
        accounting_reserve.clone(),
        String::new(),
        "## Run identity".to_string(),
        String::new(),
        format!("- Eval: `{}`", run_record.eval_id),
        format!("- Condition: `{}`", run_record.condition),
        format!(
            "- Status: `{}`",
            run_record
                .conversation
                .as_ref()
                .map(|conversation| serialized_label(&conversation.status))
                .unwrap_or_else(|| "one_shot".to_string())
        ),
        format!(
            "- Timing: {} ms; tokens: {}",
            optional_number(run_record.duration_ms),
            optional_number(run_record.total_tokens)
        ),
    ];
    if let Some(codebase) = &run_record.codebase {
        sections.push(format!(
            "- Codebase: {}{}; revision {}; branch `{}`; host-local: {}; skill sources excluded: {}",
            codebase.source.source,
            codebase
                .source
                .reference
                .as_deref()
                .map(|reference| format!("@{reference}"))
                .unwrap_or_default(),
            codebase.source.revision.as_deref().unwrap_or("unavailable"),
            codebase.source.branch,
            codebase.source.host_local,
            codebase.exclude_skill_sources,
        ));
    }
    if let Some(skill) = &run_record.skill_source {
        sections.push(format!(
            "- Skill source: {}; revision {}; branch `{}`; dirty: {}; siblings: {}",
            skill.source.source,
            skill.source.revision.as_deref().unwrap_or("unavailable"),
            skill.source.branch,
            skill.source.dirty,
            if skill.siblings.is_empty() {
                "(none)".to_string()
            } else {
                skill.siblings.join(", ")
            }
        ));
    }

    sections.extend([
        String::new(),
        "## Artifact manifest".to_string(),
        String::new(),
        format!("- This bounded bundle: {}", artifact_path(bundle_path)),
        format!("- Complete run record: {}", artifact_path(run_record_path)),
        format!(
            "- Diff metrics and file list: {}",
            artifact_path(&diff_scope_path)
        ),
        format!("- Complete captured patch: {}", artifact_path(&patch_path)),
        format!("- Raw harness outputs: {}", artifact_path(outputs_dir)),
        "- These source paths are valid in the grading iteration and may not survive teardown."
            .to_string(),
    ]);

    let prompt = bounded_fenced(
        "text",
        &run_record.prompt,
        TASK_PROMPT_BYTE_LIMIT,
        &artifact_path(run_record_path),
    );
    let final_message = bounded_fenced(
        "text",
        &run_record.final_message,
        FINAL_MESSAGE_BYTE_LIMIT,
        &artifact_path(run_record_path),
    );
    let diff = render_diff(captured_diff.as_ref(), &diff_scope_path, &patch_path)?;
    let plan = render_plan(run_record, outputs_dir);
    let conversation = render_conversation(run_record, run_record_path);
    let tools = render_tools(&run_record.tool_invocations, run_record_path);

    sections.extend([
        String::new(),
        "## `prompt`".to_string(),
        String::new(),
        prompt.content.clone(),
        String::new(),
        "## `final_message`".to_string(),
        String::new(),
        final_message.content.clone(),
    ]);
    if let Some(plan) = &plan {
        sections.extend([
            String::new(),
            "## Plan".to_string(),
            String::new(),
            plan.content.clone(),
        ]);
    }
    sections.extend([
        String::new(),
        diff.summary.content.clone(),
        String::new(),
        "### Patch".to_string(),
        String::new(),
    ]);
    let prefix = sections.join("\n");
    let suffix = format!("\n\n{}\n\n{}\n", conversation.content, tools.content);
    let fixed_bytes = prefix.len().saturating_add(suffix.len());
    if fixed_bytes >= EVIDENCE_BUNDLE_BYTE_LIMIT {
        return Err(PipelineError::Message(format!(
            "judge evidence metadata and bounded non-diff sections require {fixed_bytes} bytes, exceeding the {EVIDENCE_BUNDLE_BYTE_LIMIT}-byte bundle limit"
        )));
    }
    let patch_budget = EVIDENCE_BUNDLE_BYTE_LIMIT - fixed_bytes;
    let patch = bounded_fenced(
        "diff",
        &diff.patch,
        patch_budget,
        &artifact_path(&patch_path),
    );
    let truncated = prompt.truncated
        || final_message.truncated
        || plan.as_ref().is_some_and(|plan| plan.truncated)
        || diff.summary.truncated
        || diff.source_truncated
        || patch.truncated
        || conversation.truncated
        || tools.truncated;
    let reserved_content = format!("{prefix}{}{}", patch.content, suffix);
    let base_bytes = reserved_content.len() - accounting_reserve.len();
    // The byte count changes its own decimal width. Iterate until the rendered
    // header and the complete file length agree (at this cap, at most twice).
    let mut actual_bytes = base_bytes + evidence_accounting(0, truncated).len();
    loop {
        let next = base_bytes + evidence_accounting(actual_bytes, truncated).len();
        if next == actual_bytes {
            break;
        }
        actual_bytes = next;
    }
    let content = reserved_content.replacen(
        &accounting_reserve,
        &evidence_accounting(actual_bytes, truncated),
        1,
    );
    if content.len() != actual_bytes || content.len() > EVIDENCE_BUNDLE_BYTE_LIMIT {
        return Err(PipelineError::Message(format!(
            "judge evidence renderer produced {} bytes with an accounted size of {actual_bytes}, exceeding or disagreeing with the {EVIDENCE_BUNDLE_BYTE_LIMIT}-byte contract",
            content.len()
        )));
    }
    Ok(EvidenceBundle {
        reference: EvidenceBundleRef {
            path: artifact_path(bundle_path),
            bytes: content.len(),
            byte_limit: EVIDENCE_BUNDLE_BYTE_LIMIT,
            truncated,
        },
        content,
    })
}

fn evidence_accounting(bytes: usize, truncated: bool) -> String {
    format!(
        "- Evidence bytes: {bytes}\n- Evidence byte limit: {EVIDENCE_BUNDLE_BYTE_LIMIT}\n- Evidence truncated: {truncated}"
    )
}

fn optional_number(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unavailable".to_string())
}

fn serialized_label(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn render_diff(
    captured: Option<&CapturedDiff>,
    diff_scope_path: &Path,
    patch_path: &Path,
) -> Result<DiffEvidence, PipelineError> {
    let Some(diff) = captured else {
        return Ok(DiffEvidence {
            summary: Rendered {
                content: format!(
                    "## Diff evidence\n\n[eval-magic] diff evidence is unavailable; no {} was captured.",
                    diff_scope_path.display()
                ),
                truncated: false,
            },
            patch: "[eval-magic] captured patch is unavailable.".to_string(),
            source_truncated: false,
        });
    };
    let mut lines = vec![
        "## Diff evidence".to_string(),
        String::new(),
        format!(
            "- {} files, +{}/-{} lines, {} hunks",
            diff.metrics.files_touched,
            diff.metrics.lines_added,
            diff.metrics.lines_removed,
            diff.metrics.hunks
        ),
    ];
    if let Some(patch) = &diff.patch {
        lines.push(format!(
            "- Captured patch: {} bytes; source truncated: {}",
            patch.bytes, patch.truncated
        ));
    }
    lines.push(String::new());
    lines.push("### Changed files".to_string());
    lines.push(String::new());
    let files = if diff.files.is_empty() {
        "(none)".to_string()
    } else {
        diff.files
            .iter()
            .map(|file| {
                format!(
                    "- {} ({}, +{}/-{})",
                    file.path,
                    serialized_label(&file.status),
                    file.lines_added,
                    file.lines_removed
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let files = bounded_fenced(
        "text",
        &files,
        CHANGED_FILES_BYTE_LIMIT,
        &artifact_path(diff_scope_path),
    );
    lines.push(files.content);
    let patch = if patch_path.exists() {
        String::from_utf8_lossy(&fs::read(patch_path)?).into_owned()
    } else {
        "[eval-magic] captured patch is unavailable.".to_string()
    };
    Ok(DiffEvidence {
        summary: Rendered {
            content: lines.join("\n"),
            truncated: files.truncated,
        },
        patch,
        source_truncated: diff.patch.as_ref().is_some_and(|patch| patch.truncated),
    })
}

/// The approved plan of a plan-mode run: how it was presented and approved,
/// then the plan text the driver saved as `plan.md` beside the round outputs.
/// `None` for a run that never approved a plan.
fn render_plan(run_record: &RunRecord, outputs_dir: &Path) -> Option<Rendered> {
    let plan = run_record.conversation.as_ref()?.plan.as_ref()?;
    // The driver saved the plan inside the task environment's outputs and
    // recorded where; that is not the directory the judge stage resolves for
    // raw harness outputs, so the record's own path locates the file.
    let plan_path = plan
        .artifact_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| outputs_dir.join("plan.md"));
    let text = fs::read_to_string(&plan_path).unwrap_or_else(|_| {
        format!(
            "[eval-magic] plan artifact unavailable at {}",
            artifact_path(&plan_path)
        )
    });
    let body = bounded_fenced(
        "markdown",
        &text,
        PLAN_BYTE_LIMIT,
        &artifact_path(&plan_path),
    );
    Some(Rendered {
        content: format!(
            "Presented in round {}; approved in round {} (signal: `{}`).\n\n{}",
            plan.presented_in_round,
            plan.approved_in_round,
            serialized_label(&plan.signal),
            body.content
        ),
        truncated: body.truncated,
    })
}

fn render_conversation(run_record: &RunRecord, run_record_path: &Path) -> Rendered {
    let Some(conversation) = &run_record.conversation else {
        return Rendered {
            content: "## Conversation transcript\n\n(one-shot run; no conversation record)"
                .to_string(),
            truncated: false,
        };
    };
    let mut body = vec![
        format!(
            "Status `{}`; {} followup(s) delivered.",
            serialized_label(&conversation.status),
            conversation.delivered_followups
        ),
        String::new(),
    ];
    let mut truncated = false;
    for event in &conversation.events {
        match event {
            ConversationEvent::UserMessage {
                ordinal,
                round,
                text,
                ..
            } => {
                let text = bounded_excerpt(
                    text,
                    CONVERSATION_EVENT_BYTE_LIMIT,
                    &format!(
                        "{} conversation event {ordinal}",
                        artifact_path(run_record_path)
                    ),
                );
                truncated |= text.truncated;
                body.push(format!(
                    "round {round} user (event {ordinal})\n{}",
                    text.content
                ));
            }
            ConversationEvent::AssistantMessage {
                ordinal,
                round,
                text,
            } => {
                let text = bounded_excerpt(
                    text,
                    CONVERSATION_EVENT_BYTE_LIMIT,
                    &format!(
                        "{} conversation event {ordinal}",
                        artifact_path(run_record_path)
                    ),
                );
                truncated |= text.truncated;
                body.push(format!(
                    "round {round} assistant (event {ordinal})\n{}",
                    text.content
                ));
            }
            ConversationEvent::ToolInvocation {
                ordinal,
                round,
                name,
                ..
            } => body.push(format!("[tool {ordinal}: {name}] (round {round})")),
        }
        body.push(String::new());
    }
    let body = bounded_fenced(
        "text",
        body.join("\n").trim_end(),
        CONVERSATION_BYTE_LIMIT,
        &artifact_path(run_record_path),
    );
    Rendered {
        content: format!("## Conversation transcript\n\n{}", body.content),
        truncated: truncated || body.truncated,
    }
}

fn render_tools(invocations: &[ToolInvocation], run_record_path: &Path) -> Rendered {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for invocation in invocations {
        *counts.entry(&invocation.name).or_default() += 1;
    }
    let counts = counts
        .iter()
        .map(|(name, count)| format!("{name}: {count}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut lines = vec![
        format!(
            "{} invocation(s); by name: {}",
            invocations.len(),
            if counts.is_empty() { "(none)" } else { &counts }
        ),
        String::new(),
    ];
    let mut truncated = false;
    for invocation in invocations {
        lines.push(format!("{}. {}", invocation.ordinal, invocation.name));
        if let Some(args) = &invocation.args {
            let args = bounded_excerpt(
                &compact_json(args),
                TOOL_FIELD_BYTE_LIMIT,
                &format!(
                    "{} tool {} args",
                    artifact_path(run_record_path),
                    invocation.ordinal
                ),
            );
            truncated |= args.truncated;
            lines.push(format!("  args: {}", args.content));
        }
        if let Some(result) = &invocation.result {
            let result = bounded_excerpt(
                &compact_json(result),
                TOOL_FIELD_BYTE_LIMIT,
                &format!(
                    "{} tool {} result",
                    artifact_path(run_record_path),
                    invocation.ordinal
                ),
            );
            truncated |= result.truncated;
            lines.push(format!("  result: {}", result.content));
        }
    }
    let body = bounded_fenced(
        "text",
        &lines.join("\n"),
        TOOL_SUMMARY_BYTE_LIMIT,
        &artifact_path(run_record_path),
    );
    Rendered {
        content: format!("## Tool invocation summary\n\n{}", body.content),
        truncated: truncated || body.truncated,
    }
}

fn compact_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "<unavailable>".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    fn realistic_record() -> RunRecord {
        serde_json::from_value(json!({
            "eval_id": "add-cache",
            "condition": "with_skill",
            "skill_path": "/work/skills/tdd/SKILL.md",
            "prompt": "Add a bounded cache and cover eviction with tests.",
            "files": ["TASK.md"],
            "final_message": "Implemented the cache and its eviction tests.",
            "tool_invocations": [
                {
                    "name": "Read", "args": {"file_path": "src/cache.rs"},
                    "ordinal": 0, "result": "existing cache implementation"
                },
                {
                    "name": "Bash", "args": {"command": "cargo test cache"},
                    "ordinal": 1, "result": "test result: ok"
                }
            ],
            "total_tokens": 4200,
            "duration_ms": 12345,
            "conversation": {
                "status": "completed",
                "delivered_followups": 1,
                "events": [
                    {"type": "user_message", "ordinal": 0, "round": 1,
                     "text": "Add a bounded cache and cover eviction with tests."},
                    {"type": "tool_invocation", "ordinal": 1, "round": 1,
                     "name": "Read", "args": {"file_path": "src/cache.rs"}},
                    {"type": "assistant_message", "ordinal": 2, "round": 1,
                     "text": "Should eviction be LRU?"},
                    {"type": "user_message", "ordinal": 3, "round": 2,
                     "text": "Use the recommended LRU policy."},
                    {"type": "assistant_message", "ordinal": 4, "round": 2,
                     "text": "Implemented the cache and its eviction tests."}
                ]
            },
            "codebase": {
                "kind": "git", "source": "https://example.test/service.git",
                "ref": "main", "revision": "abc123def456", "branch": "main",
                "exclude_skill_sources": false
            },
            "skill_source": {
                "kind": "path", "source": "../skills/tdd", "branch": "dev",
                "revision": "789fed", "dirty": true, "siblings": ["verify"]
            }
        }))
        .unwrap()
    }

    #[test]
    fn bundle_contains_task_diff_conversation_tools_and_provenance() {
        let temp = tempfile::TempDir::new().unwrap();
        let run_dir = temp.path().join("eval-add-cache/with_skill");
        fs::create_dir_all(run_dir.join("outputs/turn-1")).unwrap();
        let patch = "diff --git a/src/cache.rs b/src/cache.rs\n+pub struct Cache;\n";
        fs::write(run_dir.join("diff.patch"), patch).unwrap();
        fs::write(
            run_dir.join("diff-scope.json"),
            serde_json::to_string(&json!({
                "files_touched": 2,
                "lines_added": 17,
                "lines_removed": 3,
                "hunks": 4,
                "files": [
                    {"path": "src/cache.rs", "status": "modified",
                     "lines_added": 12, "lines_removed": 3},
                    {"path": "tests/cache.rs", "status": "added",
                     "lines_added": 5, "lines_removed": 0}
                ],
                "patch": {"path": "diff.patch", "bytes": patch.len(), "truncated": false}
            }))
            .unwrap(),
        )
        .unwrap();

        let run_record_path = run_dir.join("run.json");
        let outputs_dir = run_dir.join("outputs");
        let bundle_path = run_dir.join("judge-evidence.md");
        let bundle = build_evidence_bundle(
            &realistic_record(),
            &run_record_path,
            &outputs_dir,
            &bundle_path,
        )
        .unwrap();

        for expected in [
            "Evidence truncated: false",
            "Eval: `add-cache`",
            "Condition: `with_skill`",
            "Status: `completed`",
            "https://example.test/service.git",
            "abc123def456",
            "../skills/tdd",
            "dirty: true",
            "## Diff evidence",
            "2 files, +17/-3 lines, 4 hunks",
            "src/cache.rs",
            "tests/cache.rs",
            "+pub struct Cache;",
            "## Conversation transcript",
            "round 1 user",
            "Should eviction be LRU?",
            "[tool 1: Read]",
            "## Tool invocation summary",
            "2 invocation(s)",
            "cargo test cache",
            "test result: ok",
        ] {
            assert!(bundle.content.contains(expected), "missing {expected:?}");
        }
    }

    /// A plan-mode run's judge sees the approved plan as its own section, read
    /// from the artifact the driver saved beside the round outputs.
    #[test]
    fn a_plan_mode_run_renders_its_plan_section() {
        let temp = tempfile::TempDir::new().unwrap();
        let run_dir = temp.path().join("eval-add-cache/with_skill");
        let outputs_dir = run_dir.join("outputs");
        fs::create_dir_all(outputs_dir.join("turn-1")).unwrap();
        fs::write(
            outputs_dir.join("plan.md"),
            "1. Add an LRU\n2. Cover eviction\n",
        )
        .unwrap();
        let mut record = serde_json::to_value(realistic_record()).unwrap();
        record["conversation"]["events"][0]["mode"] = json!("plan");
        record["conversation"]["events"][3]["mode"] = json!("act");
        record["conversation"]["events"][3]["origin"] = json!({"runner": "plan_approval"});
        record["conversation"]["plan"] = json!({
            "presented_in_round": 1,
            "approved_in_round": 2,
            "signal": "plan_file",
            "artifact_path": outputs_dir.join("plan.md").to_string_lossy()
        });
        let record: RunRecord = serde_json::from_value(record).unwrap();

        let bundle = build_evidence_bundle(
            &record,
            &run_dir.join("run.json"),
            &outputs_dir,
            &run_dir.join("judge-evidence.md"),
        )
        .unwrap();

        for expected in [
            "## Plan",
            "Presented in round 1",
            "approved in round 2",
            "plan_file",
            "1. Add an LRU",
            "2. Cover eviction",
        ] {
            assert!(bundle.content.contains(expected), "missing {expected:?}");
        }
        let plan_at = bundle.content.find("## Plan").unwrap();
        let transcript_at = bundle.content.find("## Conversation transcript").unwrap();
        assert!(
            plan_at < transcript_at,
            "the plan precedes the transcript it came from"
        );
    }

    /// The plan artifact lives where the driver wrote it — inside the task
    /// environment's outputs — which is not the directory the judge stage
    /// resolves for raw harness outputs. The record's own path is what locates
    /// it.
    #[test]
    fn the_plan_is_read_from_the_path_the_record_names() {
        let temp = tempfile::TempDir::new().unwrap();
        let run_dir = temp.path().join("eval-add-cache/with_skill");
        let outputs_dir = run_dir.join("outputs");
        fs::create_dir_all(&outputs_dir).unwrap();
        let elsewhere = temp.path().join("env-g1-with_skill/.eval-magic-outputs");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("plan.md"), "1. Add an LRU\n").unwrap();

        let mut record = serde_json::to_value(realistic_record()).unwrap();
        record["conversation"]["plan"] = json!({
            "presented_in_round": 1,
            "approved_in_round": 2,
            "signal": "plan_file",
            "artifact_path": elsewhere.join("plan.md").to_string_lossy()
        });
        let record: RunRecord = serde_json::from_value(record).unwrap();

        let bundle = build_evidence_bundle(
            &record,
            &run_dir.join("run.json"),
            &outputs_dir,
            &run_dir.join("judge-evidence.md"),
        )
        .unwrap();

        assert!(
            bundle.content.contains("1. Add an LRU"),
            "{}",
            bundle.content
        );
        assert!(
            !bundle.content.contains("plan artifact unavailable"),
            "{}",
            bundle.content
        );
    }

    #[test]
    fn a_run_without_a_plan_omits_the_section() {
        let temp = tempfile::TempDir::new().unwrap();
        let run_dir = temp.path().join("eval-add-cache/with_skill");
        let outputs_dir = run_dir.join("outputs");
        fs::create_dir_all(outputs_dir.join("turn-1")).unwrap();

        let bundle = build_evidence_bundle(
            &realistic_record(),
            &run_dir.join("run.json"),
            &outputs_dir,
            &run_dir.join("judge-evidence.md"),
        )
        .unwrap();

        assert!(!bundle.content.contains("## Plan"), "{}", bundle.content);
    }

    #[test]
    fn oversized_bundle_is_bounded_marked_utf8_safe_and_keeps_each_sections_tail() {
        let temp = tempfile::TempDir::new().unwrap();
        let run_dir = temp.path().join("eval-large/with_skill");
        fs::create_dir_all(run_dir.join("outputs")).unwrap();

        let patch = format!(
            "PATCH-BEGIN\n```diff\n{}PATCH-END\n",
            "+changed éééé\n".repeat(10_000)
        );
        fs::write(run_dir.join("diff.patch"), &patch).unwrap();
        let files = (0..600)
            .map(|index| {
                json!({
                    "path": format!("src/very-long-component-{index:04}/implementation.rs"),
                    "status": "modified", "lines_added": 10, "lines_removed": 2
                })
            })
            .collect::<Vec<_>>();
        fs::write(
            run_dir.join("diff-scope.json"),
            serde_json::to_string(&json!({
                "files_touched": files.len(), "lines_added": 6000,
                "lines_removed": 1200, "hunks": 600, "files": files,
                "patch": {"path": "diff.patch", "bytes": patch.len(), "truncated": false}
            }))
            .unwrap(),
        )
        .unwrap();

        let mut record = realistic_record();
        record.prompt = format!(
            "PROMPT-BEGIN\n```text\n{}PROMPT-END",
            "prompt é\n".repeat(4_000)
        );
        record.final_message = format!("FINAL-BEGIN\n{}FINAL-END", "final é\n".repeat(4_000));
        record.conversation.as_mut().unwrap().events = vec![
            ConversationEvent::UserMessage {
                ordinal: 0,
                round: 1,
                text: format!(
                    "CONVERSATION-BEGIN\n{}CONVERSATION-END",
                    "conversation é\n".repeat(4_000)
                ),
                origin: None,
                mode: None,
            },
            ConversationEvent::AssistantMessage {
                ordinal: 1,
                round: 1,
                text: "done".to_string(),
            },
        ];
        record.tool_invocations = vec![ToolInvocation {
            name: "HugeTool".to_string(),
            args: Some(json!({
                "text": format!("ARGS-BEGIN {} ARGS-END", "args-é ".repeat(2_000))
            })),
            ordinal: 0,
            result: Some(json!(format!(
                "RESULT-BEGIN {} RESULT-END",
                "result-é ".repeat(2_000)
            ))),
        }];

        let run_record_path = run_dir.join("run.json");
        let bundle = build_evidence_bundle(
            &record,
            &run_record_path,
            &run_dir.join("outputs"),
            &run_dir.join("judge-evidence.md"),
        )
        .unwrap();

        assert!(bundle.content.len() <= EVIDENCE_BUNDLE_BYTE_LIMIT);
        assert_eq!(bundle.reference.bytes, bundle.content.len());
        assert!(bundle.reference.truncated);
        assert!(
            bundle
                .content
                .contains(&format!("- Evidence bytes: {}", bundle.content.len()))
        );
        assert!(bundle.content.contains("- Evidence truncated: true"));
        assert!(bundle.content.matches("[eval-magic]").count() >= 6);
        assert!(bundle.content.contains("omitted"));
        for retained in [
            "PROMPT-BEGIN",
            "PROMPT-END",
            "FINAL-BEGIN",
            "FINAL-END",
            "PATCH-BEGIN",
            "PATCH-END",
            "CONVERSATION-BEGIN",
            "CONVERSATION-END",
            "ARGS-BEGIN",
            "ARGS-END",
            "RESULT-BEGIN",
            "RESULT-END",
            "src/very-long-component-0000/implementation.rs",
            "src/very-long-component-0599/implementation.rs",
        ] {
            assert!(
                bundle.content.contains(retained),
                "missing tail-safe {retained}"
            );
        }
        assert!(
            bundle.content.contains("````text"),
            "an embedded triple fence cannot close the generated prompt fence"
        );
        assert!(
            !bundle.content.contains('\u{fffd}'),
            "UTF-8 truncation never inserts replacement characters"
        );
    }

    #[test]
    fn fenced_sections_choose_a_non_colliding_fallback_fence() {
        let content = format!("{}\n~~~\ntail", "`".repeat(5_000));
        let rendered = bounded_fenced("text", &content, 8 * 1024, "/work/run.json");

        assert!(rendered.content.starts_with("~~~~text\n"));
        assert!(rendered.content.ends_with("\n~~~~"));
        assert!(rendered.content.contains("\n~~~\n"));
    }

    #[test]
    fn excerpt_marker_never_exceeds_a_tiny_budget() {
        let rendered = bounded_excerpt(
            &"évidence ".repeat(100),
            32,
            &format!("/work/{}", "very-long-source/".repeat(100)),
        );

        assert!(rendered.truncated);
        assert!(rendered.content.len() <= 32);
        assert!(!rendered.content.contains('\u{fffd}'));
    }
}
