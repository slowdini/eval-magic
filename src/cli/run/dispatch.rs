//! Dispatch-task and prompt assembly: turn one `(eval, condition)` pair into the
//! [`DispatchTask`] the orchestrator records in `dispatch.json`, plus the
//! human-readable `dispatch-manifest.md`.
//!
//! The prompt mirrors a real session: optional bootstrap context, the
//! harness-native available-skills block, then the eval task framing.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::adapters::{CliManifestContext, adapter_for};
use crate::core::fs::artifact_path;
use crate::core::{
    AvailableSkill, CodebaseRecord, ConditionSkill, Eval, GuardPolicyConfig, Harness,
    POSIX_TOOLING_REQUIREMENT, ResponderPolicy, ScriptedTurn, SkillSource,
};

use super::RunError;

mod prompt_components;

use prompt_components::{effective_bootstrap, render_overlay_files_block, render_skill_block};

/// One dispatchable task: the metadata the orchestrator persists per
/// `(eval, condition)`. `dispatch_prompt` is held in memory (for manifest
/// building and tests) but stripped from the serialized `dispatch.json` — the
/// prompt lives in its own file at `dispatch_prompt_path`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispatchTask {
    pub eval_id: String,
    pub condition: String,
    /// 1-based run index within a multi-run cell; absent for single-run cells.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_index: Option<u32>,
    pub skill_path: Option<String>,
    pub staged_skill_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_skill_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<ConditionSkill>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_skills: Option<Vec<AvailableSkill>>,
    pub user_prompt: String,
    #[serde(alias = "fixtures")]
    pub files: Vec<String>,
    pub outputs_dir: String,
    pub run_record_path: String,
    pub timing_path: String,
    /// Ordered scripted follow-ups; absent for one-shot tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<Vec<ScriptedTurn>>,
    /// Runner-owned completion artifact for a scripted conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_path: Option<String>,
    pub agent_description: String,
    pub dispatch_prompt_path: String,
    /// Group id this task belongs to; absent when there is exactly one group
    /// (the common no-conflict case), keeping single-group `dispatch.json`
    /// byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The agent-under-test's private cwd for this task, which the CLI dispatch
    /// recipe's `<eval-root>` placeholder resolves to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_root: Option<String>,
    /// The codebase this task's environment was built from. Carried here so the
    /// run record written at ingest names the tree the agent actually worked in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codebase: Option<CodebaseRecord>,
    /// The skill under test this task stages, as the run resolved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_source: Option<SkillSource>,
    /// The policy that derives this task's follow-up turns, when the eval
    /// declares one instead of scripting them. Recorded here so the plan names
    /// how the conversation was driven, not just what it produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responder: Option<ResponderPolicy>,
    /// Where this task's responder consultations run and are captured. It sits
    /// in the cell directory, above the env: a consultation must not be able to
    /// reach the codebase under measurement, nor pick up its `CLAUDE.md` as
    /// instructions. Absent unless the eval declares a responder, so a task
    /// without one serializes exactly as it did before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responder_dir: Option<String>,
    /// Fully expanded command policy used by the live guard and post-run audit.
    #[serde(default)]
    pub guard_policy: GuardPolicyConfig,
    /// Whether the session starts in the harness's native plan mode. Absent
    /// unless the eval declares it, so other tasks serialize as before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plan_mode: bool,
    #[serde(default, skip_serializing)]
    pub dispatch_prompt: String,
}

/// Inputs to [`build_dispatch_task`]. `harness` defaults to Claude Code.
#[derive(Debug, Clone, Default)]
pub struct DispatchTaskOpts<'a> {
    pub eval_id: &'a str,
    pub condition: &'a str,
    pub skill_path: Option<&'a str>,
    pub staged_skill_slug: Option<&'a str>,
    /// Absolute path to the staged per-condition `SKILL.md`, surfaced as an
    /// explicit fallback for a mid-session discovery miss (issue #6).
    pub staged_skill_path: Option<&'a str>,
    /// Complete treatment roster for list-authored evals. `Some(empty)` is the
    /// control arm; `None` preserves the scalar task shape.
    pub skills: Option<&'a [ConditionSkill]>,
    /// Full treatment names even in the empty control arm, used to remove every
    /// treatment reference from optional bootstrap content.
    pub treatment_names: Option<&'a [String]>,
    pub user_prompt: &'a str,
    pub files: Vec<String>,
    pub turns: Option<&'a [ScriptedTurn]>,
    /// The eval's `plan_mode` declaration.
    pub plan_mode: bool,
    pub outputs_dir: &'a str,
    pub cond_dir: &'a str,
    pub bootstrap_content: Option<&'a str>,
    pub skill_name: &'a str,
    pub available_skills: Vec<AvailableSkill>,
    pub harness: Harness,
    /// Per-run uniqueness suffix (`i<iteration>-<nonce>`) appended to the dispatch
    /// description; omitted in unit tests that exercise prompt assembly directly.
    pub run_tag: Option<&'a str>,
    /// 1-based run index within a multi-run cell (adds an `r<k>` segment to the
    /// dispatch description); `None` for single-run cells.
    pub run_index: Option<u32>,
    /// Isolation-group id this task belongs to; `None` in the single-group case
    /// (keeps the serialized task byte-identical to the pre-grouping shape).
    pub group: Option<&'a str>,
    /// The task's env dir (the agent-under-test's cwd); `None` only for legacy
    /// callers that do not carry an environment manifest.
    pub eval_root: Option<&'a str>,
    /// The codebase this task's environment was built from, if any.
    pub codebase: Option<&'a CodebaseRecord>,
    /// The skill under test this task stages, if any.
    pub skill_source: Option<&'a SkillSource>,
    /// The responder policy this eval declares, if any.
    pub responder: Option<&'a ResponderPolicy>,
}

fn render_available_skills_block_for_harness(
    harness: Harness,
    skills: &[AvailableSkill],
) -> String {
    adapter_for(harness).render_available_skills_block(skills)
}

/// Construct one dispatch task and its full prompt.
pub fn build_dispatch_task(opts: &DispatchTaskOpts) -> Result<DispatchTask, RunError> {
    let harness = opts.harness;
    // Every path this function emits — into `dispatch.json`, the manifest, and
    // the prompt the agent reads — is wire format, so render them all the same
    // way whatever the host separator is. Rendered once up front so the
    // serialized fields and the prompt text can never disagree.
    let outputs_dir = artifact_path(Path::new(opts.outputs_dir));
    let eval_root = opts.eval_root.map(|root| artifact_path(Path::new(root)));
    let skill_path = opts.skill_path.map(|p| artifact_path(Path::new(p)));
    let staged_skill_path = opts.staged_skill_path.map(|p| artifact_path(Path::new(p)));
    let mut staged_skills = opts.available_skills.clone();
    staged_skills.sort_by(|a, b| a.name.cmp(&b.name));

    let skill_block = render_skill_block(
        opts,
        skill_path.as_deref(),
        staged_skill_path.as_deref(),
        &staged_skills,
    )?;

    let overlay_files_block = render_overlay_files_block(&opts.files);

    // A condition that does not load the skill-under-test must carry zero
    // reference to it: the available-skills block auto-omits it, and a
    // user-supplied bootstrap that names it in prose is redacted here.
    let skill_absent = opts.skills.map_or_else(
        || skill_path.is_none() && opts.staged_skill_slug.is_none(),
        <[ConditionSkill]>::is_empty,
    );
    let effective_bootstrap = effective_bootstrap(opts, skill_absent);

    let mut sections: Vec<String> = Vec::new();
    if let Some(boot) = &effective_bootstrap {
        sections.push(
            [
                "<session-start-context>",
                "The following guidelines were loaded at session start by the slow-powers plugin",
                "(equivalent to the SessionStart hook firing in a real user's environment):",
                "",
                boot.trim(),
                "</session-start-context>",
                "",
            ]
            .join("\n"),
        );
    }
    let available_skills_block = render_available_skills_block_for_harness(harness, &staged_skills);
    if !available_skills_block.is_empty() {
        sections.push(format!("{available_skills_block}\n\n"));
    }

    let mut task_lines = vec![
        "You are executing a single test case for a skill evaluation framework.".to_string(),
        "Treat this as a real user request — do NOT optimize behavior for the eval.".to_string(),
    ];
    if !skill_block.is_empty() {
        task_lines.push(String::new());
        task_lines.push(skill_block);
    }
    task_lines.push(String::new());
    task_lines.push(overlay_files_block);
    if let Some(eval_root) = &eval_root {
        task_lines.push(super::scratch::context(eval_root));
    }
    task_lines.push(String::new());
    task_lines.push("Instructions:".to_string());
    task_lines.push(
        "- Work normally on the task: you may edit existing files and create new files inside the task environment."
            .to_string(),
    );
    super::scratch::push_instruction(&mut task_lines, eval_root.as_deref());
    task_lines.push("- Do not write outside the task environment.".to_string());
    task_lines.push(String::new());
    task_lines.push("User request:".to_string());
    task_lines.push(opts.user_prompt.to_string());
    sections.push(task_lines.join("\n"));

    let cond_dir = Path::new(opts.cond_dir);
    let run_seg = match opts.run_index {
        Some(k) => format!(":r{k}"),
        None => String::new(),
    };
    let agent_description = match opts.run_tag {
        Some(tag) => format!("{}:{}{run_seg}:{tag}", opts.eval_id, opts.condition),
        None => format!("{}:{}{run_seg}", opts.eval_id, opts.condition),
    };

    Ok(DispatchTask {
        eval_id: opts.eval_id.to_string(),
        condition: opts.condition.to_string(),
        run_index: opts.run_index,
        skill_path,
        staged_skill_slug: opts.staged_skill_slug.map(str::to_string),
        staged_skill_path,
        skills: opts.skills.map(<[ConditionSkill]>::to_vec),
        available_skills: opts.skills.map(|_| staged_skills),
        user_prompt: opts.user_prompt.to_string(),
        files: opts.files.clone(),
        run_record_path: artifact_path(&cond_dir.join("run.json")),
        timing_path: artifact_path(&cond_dir.join("timing.json")),
        turns: opts.turns.map(<[ScriptedTurn]>::to_vec),
        // Unconditional: the runner drives every task, so every task ends with
        // this completion artifact. Its presence is also what lets a rerun skip
        // finished work, which a one-shot task needs as much as a scripted one.
        conversation_path: Some(artifact_path(&cond_dir.join("conversation.json"))),
        agent_description,
        dispatch_prompt_path: artifact_path(&Path::new(&outputs_dir).join("dispatch-prompt.txt")),
        outputs_dir,
        group: opts.group.map(str::to_string),
        eval_root,
        codebase: opts.codebase.cloned(),
        skill_source: opts.skill_source.cloned(),
        responder: opts.responder.cloned(),
        responder_dir: opts
            .responder
            .map(|_| artifact_path(&cond_dir.join("responder"))),
        guard_policy: GuardPolicyConfig::default(),
        plan_mode: opts.plan_mode,
        dispatch_prompt: sections.join(""),
    })
}

/// Truthiness for an optional string: `Some(non-empty)` — `None` and `""` are
/// both falsy.
fn is_truthy(s: Option<&str>) -> bool {
    s.is_some_and(|s| !s.is_empty())
}

/// Filter the eval list to the `--only` / `--skip` subset (mutually exclusive).
/// Every requested id must exist; `--only` preserves config order. Errors map
/// to [`RunError::Message`].
pub fn select_evals(
    evals: &[Eval],
    only: Option<&[String]>,
    skip: Option<&[String]>,
) -> Result<Vec<Eval>, RunError> {
    if only.is_some() && skip.is_some() {
        return Err(RunError::msg("use only one of --only / --skip, not both"));
    }
    let Some(requested) = only.or(skip) else {
        return Ok(evals.to_vec());
    };
    if requested.is_empty() {
        return Err(RunError::msg("--only/--skip requires at least one eval id"));
    }

    let available: Vec<&str> = evals.iter().map(|e| e.id.as_str()).collect();
    let unknown: Vec<&str> = requested
        .iter()
        .filter(|id| !available.contains(&id.as_str()))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return Err(RunError::msg(format!(
            "unknown eval id(s): {}. Available ids: {}",
            unknown.join(", "),
            available.join(", ")
        )));
    }

    let requested_set: Vec<&str> = requested.iter().map(String::as_str).collect();
    let keep = |e: &Eval| requested_set.contains(&e.id.as_str());
    Ok(if only.is_some() {
        evals.iter().filter(|e| keep(e)).cloned().collect()
    } else {
        evals.iter().filter(|e| !keep(e)).cloned().collect()
    })
}

/// Remove the skill-under-test's "Active Skills Directory" entry (its bullet +
/// indented continuation lines) from bootstrap content, leaving siblings and the
/// heading intact.
pub fn redact_skill_from_bootstrap(content: &str, skill_name: &str) -> String {
    let bullet = Regex::new(r"^[*-]\s").unwrap();
    let indented = Regex::new(r"^\s+\S").unwrap();
    let needle = format!("`{skill_name}`");

    let mut out: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in content.split('\n') {
        if skipping {
            if indented.is_match(line) {
                continue;
            }
            skipping = false;
        }
        if bullet.is_match(line) && line.contains(&needle) {
            skipping = true;
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}

/// Read the `description:` frontmatter value (unquoted) from a skill's
/// `SKILL.md`, falling back to a placeholder.
pub fn get_skill_description(skill_path: &Path) -> String {
    const FALLBACK: &str = "No description available.";
    let Ok(content) = fs::read_to_string(skill_path) else {
        return FALLBACK.to_string();
    };
    let re = Regex::new(r"description:\s*([^\n\r]+)").unwrap();
    let Some(caps) = re.captures(&content) else {
        return FALLBACK.to_string();
    };
    let desc = caps[1].trim();
    let unquoted = if (desc.starts_with('"') && desc.ends_with('"'))
        || (desc.starts_with('\'') && desc.ends_with('\''))
    {
        desc[1..desc.len() - 1].trim()
    } else {
        desc
    };
    unquoted.to_string()
}

pub use crate::core::Mode;

/// Harness-specific knobs for the human dispatch manifest: what the runner will
/// spawn per task, and under what conditions.
#[derive(Debug, Clone, Copy)]
pub struct ManifestContext<'a> {
    pub harness: Harness,
    pub guard: bool,
    pub agent_model: Option<&'a str>,
    pub agent_env: &'a BTreeMap<String, String>,
}

/// Build the human-readable `dispatch-manifest.md`.
pub fn build_manifest(
    skill_name: &str,
    mode: Mode,
    baseline: Option<&str>,
    iteration: u32,
    timestamp: &str,
    tasks: &[DispatchTask],
    context: ManifestContext<'_>,
) -> String {
    let ManifestContext {
        harness,
        guard,
        agent_model,
        agent_env,
    } = context;
    let mode_str = match mode {
        Mode::NewSkill => "new-skill",
        Mode::Revision => "revision",
    };
    let mode_line = match baseline {
        Some(b) => format!("Mode: {mode_str} (baseline: {b})"),
        None => format!("Mode: {mode_str}"),
    };
    let mut header = vec![
        format!("# Dispatch manifest — {skill_name} iteration-{iteration}"),
        String::new(),
        mode_line,
        format!("Generated: {timestamp}"),
        format!("Total dispatches: {}", tasks.len()),
        String::new(),
        "## How to use this manifest".to_string(),
        String::new(),
        "In an agent session, read `dispatch.json` (sibling of this file) instead of this manifest. Each task has a `dispatch_prompt_path` field pointing at the file that holds the full prompt — dispatch the task with a short \"read this file and follow it\" instruction rather than inlining the prompt — plus exact paths for `run.json` and `timing.json`.".to_string(),
        String::new(),
        // Dispatch shells out to POSIX command lines, so the manifest states the
        // requirement the same way RUNBOOK.md does (issue #248).
        format!("**Requires:** {POSIX_TOOLING_REQUIREMENT}"),
        String::new(),
    ];
    header.extend([
        "## Dispatch".to_string(),
        String::new(),
        "Every task is runner-driven — one-shot and scripted alike — so one command runs the \
         whole plan from this iteration directory:"
            .to_string(),
        String::new(),
        "eval-magic dispatch --iteration <n> --harness <harness>".to_string(),
        String::new(),
        "It runs `--jobs` tasks at a time, each in its own private environment, and writes each \
         task's conversation.json. A task that already has one is skipped, so rerunning retries \
         only what did not finish. A task exceeding `--timeout` is recorded as timed out, and a \
         failing task is recorded while the rest of the batch continues. A conversation that \
         stops at a scripted gate is valid eval data; a task with no conversation.json is \
         incomplete and ingest skips it."
            .to_string(),
        String::new(),
    ]);
    // The harness section is what the descriptor still contributes: the command
    // the runner will spawn, and whatever is peculiar about reading it back.
    if let Some(lines) = adapter_for(harness).cli_manifest_section(CliManifestContext {
        guard,
        agent_model,
        agent_env,
    }) {
        header.extend(lines);
    }
    header.extend([
        "After all dispatches:".to_string(),
        String::new(),
        "1. Run `eval-magic ingest --harness <harness>` — a fixed-order chain of record-runs (assembles every task's `run.json` from `dispatch.json`, `conversation.json`, and the harness events under `outputs/turn-<n>/`, and backfills `timing.json`; never clobbers an existing record), detect-stray-writes, and grade.".to_string(),
        "2. Run `eval-magic dispatch --judges --harness <harness>` to grade the judge tasks ingest listed, then `eval-magic finalize` for the benchmark.".to_string(),
        String::new(),
        "## Dispatches".to_string(),
        String::new(),
    ]);
    let header = header.join("\n");

    let entries = tasks
        .iter()
        .map(|t| {
            let run_seg = t
                .run_index
                .map(|k| format!(" / run-{k}"))
                .unwrap_or_default();
            let mut lines = vec![
                format!("### {} / {}{run_seg}", t.eval_id, t.condition),
                String::new(),
                format!("- run.json:    {}", t.run_record_path),
                format!("- timing.json: {}", t.timing_path),
            ];
            if let Some(path) = &t.conversation_path {
                lines.push(format!("- conversation.json: {path}"));
            }
            lines.extend([
                String::new(),
                "```".to_string(),
                t.dispatch_prompt.clone(),
                "```".to_string(),
                String::new(),
            ]);
            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!("{header}{entries}")
}

#[cfg(test)]
mod tests {
    use super::*;

    mod conversation;
    mod guard_policy;

    fn mk_evals(ids: &[&str]) -> Vec<Eval> {
        ids.iter()
            .map(|id| Eval {
                id: (*id).to_string(),
                prompt: format!("p-{id}"),
                expected_output: format!("o-{id}"),
                files: None,
                files_root: None,
                assertions: None,
                skill_should_trigger: None,
                runs: None,
                turns: None,
                codebase: None,
                responder: None,
                guard: None,
                plan_mode: false,
            })
            .collect()
    }

    #[test]
    fn run_index_adds_r_segment_to_agent_description() {
        let task = build_dispatch_task(&DispatchTaskOpts {
            eval_id: "e1",
            condition: "with_skill",
            cond_dir: "/work/eval-e1/with_skill/run-2",
            run_tag: Some("i1-abc"),
            run_index: Some(2),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(task.agent_description, "e1:with_skill:r2:i1-abc");
        assert_eq!(task.run_index, Some(2));
        assert_eq!(
            task.run_record_path,
            "/work/eval-e1/with_skill/run-2/run.json"
        );

        let flat = build_dispatch_task(&DispatchTaskOpts {
            eval_id: "e1",
            condition: "with_skill",
            cond_dir: "/work/eval-e1/with_skill",
            run_tag: Some("i1-abc"),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(flat.agent_description, "e1:with_skill:i1-abc");
        assert_eq!(flat.run_index, None);
    }

    fn skill(name: &str, description: &str) -> AvailableSkill {
        AvailableSkill {
            name: name.into(),
            path: format!("/x/{name}/SKILL.md"),
            description: description.into(),
        }
    }

    fn base_opts<'a>() -> DispatchTaskOpts<'a> {
        DispatchTaskOpts {
            eval_id: "e1",
            condition: "with_skill",
            staged_skill_slug: Some("slow-powers-eval-1-with_skill__foo"),
            user_prompt: "do the thing",
            outputs_dir: "/tmp/out",
            cond_dir: "/tmp/cond",
            skill_name: "foo",
            ..Default::default()
        }
    }

    // ── select_evals ──────────────────────────────────────────────────────

    #[test]
    fn select_returns_full_list_when_no_flags() {
        let evals = mk_evals(&["a", "b", "c"]);
        assert_eq!(select_evals(&evals, None, None).unwrap(), evals);
    }

    #[test]
    fn only_keeps_named_ids_in_config_order() {
        let evals = mk_evals(&["a", "b", "c"]);
        let only = vec!["c".to_string(), "a".to_string()];
        let got = select_evals(&evals, Some(&only), None).unwrap();
        assert_eq!(
            got.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "c"]
        );
    }

    #[test]
    fn skip_drops_named_ids() {
        let evals = mk_evals(&["a", "b", "c"]);
        let skip = vec!["b".to_string()];
        let got = select_evals(&evals, None, Some(&skip)).unwrap();
        assert_eq!(
            got.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "c"]
        );
    }

    #[test]
    fn unknown_id_lists_unknown_and_available() {
        let evals = mk_evals(&["a", "b"]);
        let only = vec!["a".to_string(), "nope".to_string()];
        let err = select_evals(&evals, Some(&only), None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown eval id(s): nope. Available ids: a, b"
        );
    }

    #[test]
    fn both_only_and_skip_errors() {
        let evals = mk_evals(&["a", "b"]);
        let only = vec!["a".to_string()];
        let skip = vec!["b".to_string()];
        let err = select_evals(&evals, Some(&only), Some(&skip)).unwrap_err();
        assert!(err.to_string().contains("only one of --only / --skip"));
    }

    #[test]
    fn empty_id_list_errors() {
        let evals = mk_evals(&["a", "b"]);
        let only: Vec<String> = vec![];
        let err = select_evals(&evals, Some(&only), None).unwrap_err();
        assert!(err.to_string().contains("at least one eval id"));
    }

    // ── build_dispatch_task: bootstrap injection ──────────────────────────

    #[test]
    fn prompt_allows_task_edits_without_requesting_agent_authored_framework_artifacts() {
        let task = build_dispatch_task(&DispatchTaskOpts {
            eval_root: Some("/tmp/env"),
            ..base_opts()
        })
        .unwrap();
        let prompt = task.dispatch_prompt;

        assert!(prompt.contains("Task environment: /tmp/env"));
        assert!(prompt.contains("edit existing files and create new files inside"));
        assert!(prompt.contains("Do not write outside the task environment."));
        assert!(!prompt.contains("Framework output directory:"));
        assert!(!prompt.contains("framework artifacts"));
        assert!(!prompt.contains("final-message.md"));
        assert!(!prompt.contains("Write any files you produce into the output directory."));
        assert!(!prompt.contains("Do not write outside the output directory."));
    }

    #[test]
    fn prepends_session_start_context_for_claude_code() {
        let task = build_dispatch_task(&DispatchTaskOpts {
            bootstrap_content: Some("BOOT-LOADED"),
            ..base_opts()
        })
        .unwrap();
        assert!(task.dispatch_prompt.starts_with("<session-start-context>"));
        assert!(task.dispatch_prompt.contains("BOOT-LOADED"));
        assert!(task.dispatch_prompt.contains("</session-start-context>"));
    }

    #[test]
    fn omits_session_start_context_when_null_and_nothing_staged() {
        let task = build_dispatch_task(&base_opts()).unwrap();
        assert!(!task.dispatch_prompt.contains("<session-start-context>"));
    }

    #[test]
    fn emits_harness_native_available_skills_block_when_bootstrap_null() {
        let task = build_dispatch_task(&DispatchTaskOpts {
            available_skills: vec![skill("foo", "the foo skill")],
            ..base_opts()
        })
        .unwrap();
        assert!(!task.dispatch_prompt.contains("<session-start-context>"));
        assert!(
            task.dispatch_prompt
                .contains("The following skills are available for use with the Skill tool:")
        );
        assert!(task.dispatch_prompt.contains("- foo: the foo skill"));
        assert!(!task.dispatch_prompt.contains("staged and discoverable"));
        assert!(!task.dispatch_prompt.contains("*Trigger:*"));
        assert!(!task.dispatch_prompt.contains("loaded at session start"));
    }

    #[test]
    fn available_skills_block_is_its_own_section_after_bootstrap() {
        let task = build_dispatch_task(&DispatchTaskOpts {
            bootstrap_content: Some("BOOT-LOADED"),
            available_skills: vec![skill("foo", "the foo skill")],
            ..base_opts()
        })
        .unwrap();
        let prompt = &task.dispatch_prompt;
        let ssc_end = prompt.find("</session-start-context>").unwrap();
        let list_idx = prompt
            .find("The following skills are available for use with the Skill tool:")
            .unwrap();
        let boot_idx = prompt.find("BOOT-LOADED").unwrap();
        assert!(boot_idx < ssc_end);
        assert!(list_idx > ssc_end);
    }

    #[test]
    fn task_carries_group_and_eval_root_when_set_and_omits_when_absent() {
        let with = build_dispatch_task(&DispatchTaskOpts {
            group: Some("g2"),
            eval_root: Some("/work/env-g2-with_skill"),
            ..base_opts()
        })
        .unwrap();
        assert_eq!(with.group.as_deref(), Some("g2"));
        assert_eq!(with.eval_root.as_deref(), Some("/work/env-g2-with_skill"));
        let out = serde_json::to_value(&with).unwrap();
        assert_eq!(
            out.get("group"),
            Some(&serde_json::Value::String("g2".into()))
        );
        assert_eq!(
            out.get("eval_root"),
            Some(&serde_json::Value::String("/work/env-g2-with_skill".into()))
        );

        // Single-group default: both omitted, keeping dispatch.json byte-identical.
        let without = build_dispatch_task(&base_opts()).unwrap();
        assert_eq!(without.group, None);
        assert_eq!(without.eval_root, None);
        let out = serde_json::to_value(&without).unwrap();
        assert!(out.get("group").is_none());
        assert!(out.get("eval_root").is_none());
    }

    /// Every task is runner-driven, so every task has the completion artifact
    /// the driver writes and `dispatch` reads to decide what a rerun may skip.
    /// Gating this on `turns` would leave one-shot tasks with no resume marker.
    #[test]
    fn every_task_carries_a_conversation_path_whether_or_not_it_is_scripted() {
        let turns = vec![ScriptedTurn {
            prompt: "Use US timezones.".into(),
            deliver_when: crate::core::DeliverWhen::AgentAsks,
            agent_response_matches: None,
        }];
        let scripted = build_dispatch_task(&DispatchTaskOpts {
            turns: Some(&turns),
            ..base_opts()
        })
        .unwrap();
        let one_shot = build_dispatch_task(&base_opts()).unwrap();
        assert_eq!(
            scripted.conversation_path.as_deref(),
            Some("/tmp/cond/conversation.json")
        );
        assert_eq!(
            one_shot.conversation_path.as_deref(),
            Some("/tmp/cond/conversation.json"),
            "a one-shot task needs the same completion artifact"
        );
    }

    #[test]
    fn dispatch_prompt_path_under_outputs_dir() {
        let task = build_dispatch_task(&base_opts()).unwrap();
        assert_eq!(task.dispatch_prompt_path, "/tmp/out/dispatch-prompt.txt");
        assert_eq!(task.run_record_path, "/tmp/cond/run.json");
        assert_eq!(task.timing_path, "/tmp/cond/timing.json");
    }

    const SAMPLE_DIRECTORY: &str = "## Active Skills Directory\n\n* **`test-driven-development`**\n  * *Trigger:* Use whenever implementing code.\n* **`systematic-debugging`**\n  * *Trigger:* Use when debugging.";

    #[test]
    fn redact_removes_skill_under_test_entry() {
        let redacted = redact_skill_from_bootstrap(SAMPLE_DIRECTORY, "test-driven-development");
        assert!(!redacted.contains("test-driven-development"));
        assert!(!redacted.contains("Use whenever implementing code."));
        assert!(redacted.contains("systematic-debugging"));
        assert!(redacted.contains("Use when debugging."));
        assert!(redacted.contains("## Active Skills Directory"));
    }

    #[test]
    fn redacts_skill_under_test_in_skill_absent_condition() {
        let without_skill = build_dispatch_task(&DispatchTaskOpts {
            condition: "without_skill",
            staged_skill_slug: None,
            skill_name: "test-driven-development",
            bootstrap_content: Some(SAMPLE_DIRECTORY),
            ..base_opts()
        })
        .unwrap();
        assert!(
            !without_skill
                .dispatch_prompt
                .contains("test-driven-development")
        );
        assert!(
            without_skill
                .dispatch_prompt
                .contains("systematic-debugging")
        );

        let with_skill = build_dispatch_task(&DispatchTaskOpts {
            condition: "with_skill",
            staged_skill_slug: Some("slow-powers-eval-1-with_skill__test-driven-development"),
            skill_name: "test-driven-development",
            bootstrap_content: Some(SAMPLE_DIRECTORY),
            ..base_opts()
        })
        .unwrap();
        assert!(
            with_skill
                .dispatch_prompt
                .contains("test-driven-development")
        );
    }

    #[test]
    fn names_staged_slug_without_instructing_invocation() {
        let task = build_dispatch_task(&DispatchTaskOpts {
            bootstrap_content: Some("BOOT-LOADED"),
            ..base_opts()
        })
        .unwrap();
        let p = &task.dispatch_prompt;
        assert!(p.contains("slow-powers-eval-1-with_skill__foo"));
        assert!(!p.contains("invoke that slug"));
        assert!(!p.contains("if the skill applies"));
        assert!(!p.contains("under evaluation"));
        assert!(!p.contains("plugin loaded"));
        assert!(!p.contains("rather than the bare name"));
    }

    #[test]
    fn adds_staged_snapshot_fallback_claude_code() {
        let staged = "/repo/.claude/skills/slow-powers-eval-1-with_skill__foo/SKILL.md";
        let task = build_dispatch_task(&DispatchTaskOpts {
            staged_skill_path: Some(staged),
            ..base_opts()
        })
        .unwrap();
        assert!(
            task.dispatch_prompt
                .contains("registered under the identifier `slow-powers-eval-1-with_skill__foo`")
        );
        assert!(
            task.dispatch_prompt
                .contains("If the Skill tool cannot resolve that identifier")
        );
        assert!(
            task.dispatch_prompt
                .contains(&format!("read the skill from `{staged}` instead."))
        );
    }

    #[test]
    fn codex_flavored_fallback_wording() {
        let staged = "/repo/.agents/skills/slow-powers-eval-1-with_skill__foo/SKILL.md";
        let task = build_dispatch_task(&DispatchTaskOpts {
            harness: Harness::resolve("codex").unwrap(),
            staged_skill_path: Some(staged),
            ..base_opts()
        })
        .unwrap();
        assert!(
            task.dispatch_prompt
                .contains("discoverable as a Codex skill")
        );
        assert!(
            task.dispatch_prompt
                .contains("If it does not load as a Codex skill")
        );
        assert!(
            task.dispatch_prompt
                .contains(&format!("read the skill from `{staged}` instead."))
        );
    }

    #[test]
    fn opencode_flavored_fallback_wording() {
        let staged = "/repo/.opencode/skills/slow-powers-eval-1-with-skill-foo/SKILL.md";
        let task = build_dispatch_task(&DispatchTaskOpts {
            harness: Harness::resolve("opencode").unwrap(),
            staged_skill_path: Some(staged),
            ..base_opts()
        })
        .unwrap();
        assert!(
            task.dispatch_prompt
                .contains("discoverable as an OpenCode skill")
        );
        assert!(
            task.dispatch_prompt
                .contains("If it does not load as an OpenCode skill")
        );
        assert!(
            task.dispatch_prompt
                .contains(&format!("read the skill from `{staged}` instead."))
        );
    }

    #[test]
    fn opencode_available_skills_block_uses_xml() {
        let task = build_dispatch_task(&DispatchTaskOpts {
            harness: Harness::resolve("opencode").unwrap(),
            available_skills: vec![skill("foo", "the foo skill")],
            ..base_opts()
        })
        .unwrap();
        let p = &task.dispatch_prompt;
        assert!(p.contains("<available_skills>"));
        assert!(p.contains("<name>foo</name>"));
        assert!(p.contains("<description>the foo skill</description>"));
        assert!(!p.contains("The following skills are available for use with the Skill tool:"));
    }

    #[test]
    fn omits_fallback_when_no_staged_path() {
        let task = build_dispatch_task(&base_opts()).unwrap();
        assert!(
            task.dispatch_prompt
                .contains("registered under the identifier `slow-powers-eval-1-with_skill__foo`")
        );
        assert!(!task.dispatch_prompt.contains("read the skill from"));
    }

    #[test]
    fn without_skill_realistic_env_no_announcing_commentary() {
        let task = build_dispatch_task(&DispatchTaskOpts {
            staged_skill_slug: None,
            bootstrap_content: Some("BOOT-LOADED"),
            ..base_opts()
        })
        .unwrap();
        assert!(!task.dispatch_prompt.contains("No skill is loaded"));
        assert!(
            !task
                .dispatch_prompt
                .to_lowercase()
                .contains("not available")
        );
        assert!(!task.dispatch_prompt.contains("under evaluation"));
    }

    #[test]
    fn without_skill_without_bootstrap_keeps_legacy_wording() {
        let task = build_dispatch_task(&DispatchTaskOpts {
            staged_skill_slug: None,
            ..base_opts()
        })
        .unwrap();
        assert!(task.dispatch_prompt.contains("No skill is loaded"));
    }
}
