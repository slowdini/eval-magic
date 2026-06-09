//! `command_run` — the top-level orchestrator that builds an iteration's
//! workspace: validate the request, stage the skill(s), generate every
//! `(eval, condition)` dispatch task, write `dispatch.json` /
//! `dispatch-manifest.md` / `conditions.json`, optionally arm the write guard,
//! and preflight plugin shadows.
//!
//! Ports `run.ts:566-957` plus its small helpers (`validateHarnessRunOptions`,
//! `nextIteration`, `conditionNamesFor`, `stagingDiscoveryWarning`,
//! `resolvePlanModeProfile`). The staging and dispatch mechanics live in the
//! sibling [`super::staging`] / [`super::dispatch`] modules; this file is the
//! coordinator that wires them together.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::adapters::{detect_plugin_shadows, format_shadow_banner, resolve_config_dir};
use crate::core::{AvailableSkill, ConditionEntry, ConditionsRecord, Harness, Mode, RunContext};
use crate::pipeline::io::now_iso8601;
use crate::sandbox::{install_guard, teardown_guard};
use crate::validation::validate_evals_config;

use super::dispatch::{
    DispatchTaskOpts, build_dispatch_task, build_manifest, copy_fixtures, get_skill_description,
    select_evals,
};
use super::staging::{
    StageSiblingOpts, StageSkillOpts, cleanup_staged_skills, register_staged_skill_for_cleanup,
    skills_dir_for_harness, stage_sibling_skills, stage_skill_for_harness,
};
use super::{RunError, write_json};

/// Run options parsed from the `run` subcommand flags (everything beyond the
/// shared skill/workspace/harness context, which lives in [`RunContext`]).
#[derive(Debug, Clone, Default)]
pub struct RunOptions<'a> {
    pub mode: Option<&'a str>,
    pub baseline: Option<&'a str>,
    pub only: Option<&'a [String]>,
    pub skip: Option<&'a [String]>,
    pub iteration: Option<u32>,
    pub dry_run: bool,
    pub no_stage: bool,
    pub guard: bool,
    pub stage_name: Option<&'a str>,
    pub plan_mode: bool,
}

/// The two condition names for a comparison mode. Ports `conditionNamesFor`.
fn condition_names_for(mode: Mode) -> (&'static str, &'static str) {
    match mode {
        Mode::NewSkill => ("with_skill", "without_skill"),
        Mode::Revision => ("old_skill", "new_skill"),
    }
}

/// The next iteration number for a skill's workspace dir: the explicit override,
/// else one past the highest existing `iteration-<n>`. Ports `nextIteration`.
fn next_iteration(workspace_skill_dir: &Path, override_n: Option<u32>) -> u32 {
    if let Some(n) = override_n {
        return n;
    }
    let Ok(entries) = fs::read_dir(workspace_skill_dir) else {
        return 1;
    };
    let max = entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_prefix("iteration-")
                .and_then(|s| s.parse::<u32>().ok())
        })
        .max();
    max.map_or(1, |m| m + 1)
}

/// Build-time heads-up for the same-session staging limitation on Claude Code
/// (issue #7): `run` stages mid-session, but in-process Task subagents inherit a
/// skill registry fixed at session start, so they never discover the staged
/// skills. Returns the warning, or `None` when it does not apply (staging off, or
/// Codex's fresh-process path). Ports `stagingDiscoveryWarning`.
pub fn staging_discovery_warning(harness: Harness, no_stage: bool) -> Option<String> {
    if no_stage || harness != Harness::ClaudeCode {
        return None;
    }
    Some(
        [
            "\n⚠ Staged skill discovery requires the staged skills to exist at session start,",
            "  but `run` stages them mid-session. Subagents dispatched from this same session",
            "  (in-process via the Task tool) won't discover them, so every with-skill arm falls",
            "  back. Use one of the two valid paths:",
            "    1. dispatch the subagents from a fresh Claude Code session started after the",
            "       workspace is built, so the staged skills are discovered at session start; or",
            "    2. re-run with --no-stage to inline each condition's SKILL.md into the dispatch",
            "       prompt (correct when the description: frontmatter is unchanged, since there's",
            "       nothing to measure on the discovery axis).",
            "  Either way, run detect-stray-writes (folded into `ingest`) before trusting a staged",
            "  result — it flags live-source reads that reveal a discovery miss after the fact.",
        ]
        .join("\n"),
    )
}

/// Resolve the verbatim plan-mode procedure profile for a harness (issue #142).
/// The profile is a compile-time bundled asset (mirroring the schema embedding in
/// `validation`); a harness without one gets a clear error rather than a silent
/// no-op. Ports `resolvePlanModeProfile`.
fn resolve_plan_mode_profile(harness: Harness) -> Result<&'static str, RunError> {
    match harness {
        Harness::ClaudeCode => Ok(include_str!("../../../profiles/claude-code/plan-mode.md")),
        Harness::Codex => Err(RunError::msg(
            "--plan-mode: no plan-mode profile exists for harness 'codex'. This is a Claude-tier \
             fidelity layer; a harness without a profile leaves the portable dispatch contract \
             unchanged.",
        )),
    }
}

/// Reject the Claude-tier features Codex support does not yet cover. Ports
/// `validateHarnessRunOptions`.
fn validate_harness_run_options(opts: &RunOptions, ctx: &RunContext) -> Result<(), RunError> {
    if ctx.harness != Harness::Codex {
        return Ok(());
    }
    let mut unsupported: Vec<&str> = Vec::new();
    if opts.guard {
        unsupported.push("--guard");
    }
    if ctx.bootstrap_path.is_some() && opts.no_stage {
        unsupported.push("--bootstrap with --no-stage");
    }
    if opts.plan_mode {
        unsupported.push("--plan-mode");
    }
    if opts.stage_name.is_some() && opts.no_stage {
        unsupported.push("--stage-name with --no-stage");
    }
    if unsupported.is_empty() {
        Ok(())
    } else {
        Err(RunError::msg(format!(
            "Codex harness support does not cover every Claude-tier feature yet. Unsupported for \
             Codex: {}.",
            unsupported.join(", ")
        )))
    }
}

/// A per-run nonce (`<millis-base36>-<6 hex>`) that namespaces dispatch
/// descriptions so transcripts can't collide across iterations sharing one parent
/// session's subagents dir. The TS original uses `crypto.randomBytes`; with no
/// RNG crate, the low bits of the sub-millisecond clock supply the entropy —
/// enough, since the base36 millis prefix already differs between runs.
fn make_run_nonce() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}-{:06x}",
        to_base36(now.as_millis() as u64),
        now.subsec_nanos() & 0x00ff_ffff
    )
}

fn to_base36(mut n: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::NewSkill => "new-skill",
        Mode::Revision => "revision",
    }
}

fn harness_label(harness: Harness) -> &'static str {
    match harness {
        Harness::ClaudeCode => "claude-code",
        Harness::Codex => "codex",
    }
}

/// Build the iteration workspace and dispatch plan for a run. Ports `commandRun`.
pub fn command_run(ctx: &RunContext, opts: &RunOptions) -> Result<(), RunError> {
    let mode = match opts.mode {
        Some("new-skill") => Mode::NewSkill,
        Some("revision") => Mode::Revision,
        Some(other) => return Err(RunError::msg(format!("unknown --mode: {other}"))),
        None => return Err(RunError::msg("--mode required: new-skill | revision")),
    };
    if mode == Mode::Revision && opts.baseline.is_none() {
        return Err(RunError::msg("revision mode requires --baseline <label>"));
    }
    validate_harness_run_options(opts, ctx)?;

    let skill_md_path = ctx.skill_subdir.join("SKILL.md");
    if !skill_md_path.exists() {
        return Err(RunError::msg(format!(
            "skill not found: {}",
            skill_md_path.display()
        )));
    }
    let skill_md = skill_md_path.to_string_lossy().into_owned();

    let evals_path = ctx.skill_subdir.join("evals").join("evals.json");
    if !evals_path.exists() {
        return Err(RunError::msg(format!(
            "evals.json not found: {}",
            evals_path.display()
        )));
    }
    let value: Value = serde_json::from_str(&fs::read_to_string(&evals_path)?)?;
    let config = validate_evals_config(&value, &evals_path.to_string_lossy())?;
    if config.skill_name != ctx.skill_name {
        eprintln!(
            "warning: evals.json skill_name ({}) does not match the skill folder ({}). Proceeding with {}.",
            config.skill_name, ctx.skill_name, ctx.skill_name
        );
    }

    let selected_evals = select_evals(&config.evals, opts.only, opts.skip)?;

    let workspace_skill_dir = ctx.workspace_root.join(&ctx.skill_name);
    let iteration = next_iteration(&workspace_skill_dir, opts.iteration);
    let iteration_dir = workspace_skill_dir.join(format!("iteration-{iteration}"));
    let run_nonce = make_run_nonce();
    let run_tag = format!("i{iteration}-{run_nonce}");

    if iteration_dir.exists() && opts.iteration.is_none() {
        return Err(RunError::msg(format!(
            "iteration-{iteration} already exists; pass --iteration to overwrite explicitly"
        )));
    }

    let (cond_a, cond_b) = condition_names_for(mode);
    let (skill_path_a, skill_path_b): (Option<String>, Option<String>) = match mode {
        Mode::NewSkill => (Some(skill_md.clone()), None),
        Mode::Revision => {
            let baseline = opts.baseline.expect("checked above");
            let baseline_skill = workspace_skill_dir
                .join("snapshots")
                .join(baseline)
                .join("SKILL.md");
            if !baseline_skill.exists() {
                return Err(RunError::msg(format!(
                    "baseline snapshot not found: {}\n  Run: skill-eval snapshot --skill {} --skill-dir {} --label {} (before editing)",
                    baseline_skill.display(),
                    ctx.skill_name,
                    ctx.skill_dir.display(),
                    baseline
                )));
            }
            (
                Some(baseline_skill.to_string_lossy().into_owned()),
                Some(skill_md.clone()),
            )
        }
    };

    println!(
        "Preparing {} iteration-{iteration} ({})",
        ctx.skill_name,
        mode_str(mode)
    );
    println!(
        "  {cond_a}: {}",
        skill_path_a.as_deref().unwrap_or("(no skill)")
    );
    println!(
        "  {cond_b}: {}",
        skill_path_b.as_deref().unwrap_or("(no skill)")
    );
    if selected_evals.len() != config.evals.len() {
        let (flag, ids) = match (opts.only, opts.skip) {
            (Some(ids), _) => ("--only", ids),
            (_, skip) => ("--skip", skip.unwrap_or(&[])),
        };
        println!(
            "  selection: {} of {} evals ({flag} {})",
            selected_evals.len(),
            config.evals.len(),
            ids.join(", ")
        );
    }
    if opts.no_stage {
        println!(
            "  staging: disabled (--no-stage) — skills will be inlined into dispatch_prompt for harnesses without project-local skill discovery"
        );
    }

    fs::create_dir_all(&iteration_dir)?;
    fs::copy(&skill_md_path, iteration_dir.join("skill-snapshot.md"))?;

    // Always disarm a prior run's guard before re-staging, so a crashed run can't
    // leave the write-blocking hook armed across runs.
    teardown_guard(&ctx.stage_root);

    if !opts.no_stage {
        cleanup_staged_skills(&ctx.stage_root, ctx.harness)?;
        stage_sibling_skills(&StageSiblingOpts {
            skill_under_test: &ctx.skill_name,
            skills_source_dir: &ctx.skill_dir,
            repo_root: &ctx.stage_root,
            harness: ctx.harness,
        })?;
    }

    if let Some(warning) = staging_discovery_warning(ctx.harness, opts.no_stage) {
        eprintln!("{warning}");
    }

    let bootstrap_content = match &ctx.bootstrap_path {
        Some(path) => Some(fs::read_to_string(path)?),
        None => None,
    };

    let plan_mode_content = if opts.plan_mode {
        let profile = resolve_plan_mode_profile(ctx.harness)?;
        println!(
            "  plan-mode: injecting {} plan-mode profile as operating context (issue #142; necessary-not-sufficient fidelity layer)",
            harness_label(ctx.harness)
        );
        Some(profile.to_string())
    } else {
        None
    };

    // Sibling skill metadata, shared across conditions. Empty when --no-stage.
    let sibling_skills: Vec<AvailableSkill> = if opts.no_stage {
        Vec::new()
    } else {
        ctx.sibling_skill_names
            .iter()
            .map(|name| AvailableSkill {
                name: name.clone(),
                path: skills_dir_for_harness(&ctx.stage_root, ctx.harness)
                    .join(name)
                    .join("SKILL.md")
                    .to_string_lossy()
                    .into_owned(),
                description: get_skill_description(&ctx.skill_dir.join(name).join("SKILL.md")),
            })
            .collect()
    };

    // --stage-name overrides the conspicuous slug with a verbatim name; it targets
    // the single staging condition, so reject the both-stage case and refuse to
    // clobber a pre-existing dir.
    if let Some(stage_name) = opts.stage_name
        && !opts.no_stage
    {
        if skill_path_a.is_some() && skill_path_b.is_some() {
            return Err(RunError::msg(
                "--stage-name is only supported when exactly one condition stages the skill (e.g. --mode new-skill); both conditions stage here.",
            ));
        }
        let target = skills_dir_for_harness(&ctx.stage_root, ctx.harness).join(stage_name);
        if target.exists() {
            return Err(RunError::msg(format!(
                "--stage-name \"{stage_name}\": {} already exists; refusing to clobber it. Remove it or choose a different name.",
                target.display()
            )));
        }
    }

    let stage_for =
        |cond_name: &str, cond_skill_path: Option<&str>| -> Result<Option<String>, RunError> {
            let Some(path) = cond_skill_path.filter(|_| !opts.no_stage) else {
                return Ok(None);
            };
            let content = fs::read_to_string(path)?;
            let slug = stage_skill_for_harness(&StageSkillOpts {
                content: &content,
                iteration,
                condition: cond_name,
                skill_name: &ctx.skill_name,
                repo_root: &ctx.stage_root,
                assets_dir: Path::new(path).parent(),
                stage_name_override: opts.stage_name,
                harness: ctx.harness,
            })?;
            Ok(Some(slug))
        };

    let cond_a_slug = stage_for(cond_a, skill_path_a.as_deref())?;
    let cond_b_slug = stage_for(cond_b, skill_path_b.as_deref())?;

    // A custom-named dir isn't caught by the prefix scan; record it for cleanup.
    if let Some(stage_name) = opts.stage_name
        && (cond_a_slug.as_deref() == Some(stage_name)
            || cond_b_slug.as_deref() == Some(stage_name))
    {
        register_staged_skill_for_cleanup(&ctx.stage_root, stage_name, ctx.harness)?;
    }

    let conditions = ConditionsRecord {
        mode,
        baseline: opts.baseline.map(str::to_string),
        conditions: vec![
            ConditionEntry {
                name: cond_a.to_string(),
                skill_path: skill_path_a.clone(),
                staged_skill_slug: Some(cond_a_slug.clone()),
            },
            ConditionEntry {
                name: cond_b.to_string(),
                skill_path: skill_path_b.clone(),
                staged_skill_slug: Some(cond_b_slug.clone()),
            },
        ],
        timestamp: now_iso8601(),
        harness: Some(ctx.harness),
        run_nonce: Some(run_nonce.clone()),
    };
    write_json(&iteration_dir.join("conditions.json"), &conditions)?;

    let staged_skill_path_for = |cond_slug: Option<&str>| -> Option<String> {
        cond_slug.map(|slug| {
            skills_dir_for_harness(&ctx.stage_root, ctx.harness)
                .join(slug)
                .join("SKILL.md")
                .to_string_lossy()
                .into_owned()
        })
    };

    // availableSkills for a condition = siblings + the skill-under-test when that
    // condition loads it. Empty when nothing was staged.
    let available_skills_for =
        |cond_skill_path: Option<&str>, cond_slug: Option<&str>| -> Vec<AvailableSkill> {
            if opts.no_stage {
                return Vec::new();
            }
            let mut skills = sibling_skills.clone();
            if let Some(csp) = cond_skill_path {
                let name = match (ctx.harness, cond_slug) {
                    (Harness::Codex, Some(slug)) => slug.to_string(),
                    _ => ctx.skill_name.clone(),
                };
                skills.push(AvailableSkill {
                    name,
                    path: staged_skill_path_for(cond_slug).unwrap_or_else(|| csp.to_string()),
                    description: get_skill_description(Path::new(csp)),
                });
            }
            skills
        };

    let mut tasks = Vec::new();
    for ev in &selected_evals {
        let eval_dir = iteration_dir.join(format!("eval-{}", ev.id));
        fs::create_dir_all(&eval_dir)?;

        for (cond_name, cond_skill_path, cond_slug) in [
            (cond_a, skill_path_a.as_deref(), cond_a_slug.as_deref()),
            (cond_b, skill_path_b.as_deref(), cond_b_slug.as_deref()),
        ] {
            let cond_dir = eval_dir.join(cond_name);
            let outputs_dir = cond_dir.join("outputs");
            fs::create_dir_all(&outputs_dir)?;

            let fixtures = copy_fixtures(ev, &ctx.skill_subdir, &cond_dir)?;
            let staged_path = staged_skill_path_for(cond_slug);
            let available_skills = available_skills_for(cond_skill_path, cond_slug);
            let outputs_dir_str = outputs_dir.to_string_lossy().into_owned();
            let cond_dir_str = cond_dir.to_string_lossy().into_owned();

            tasks.push(build_dispatch_task(&DispatchTaskOpts {
                eval_id: &ev.id,
                condition: cond_name,
                skill_path: cond_skill_path,
                staged_skill_slug: cond_slug,
                staged_skill_path: staged_path.as_deref(),
                user_prompt: &ev.prompt,
                fixtures,
                outputs_dir: &outputs_dir_str,
                cond_dir: &cond_dir_str,
                bootstrap_content: bootstrap_content.as_deref(),
                plan_mode_content: plan_mode_content.as_deref(),
                skill_name: &ctx.skill_name,
                available_skills,
                harness: ctx.harness,
                run_tag: Some(&run_tag),
            })?);
        }
    }

    let manifest_path = iteration_dir.join("dispatch-manifest.md");
    fs::write(
        &manifest_path,
        build_manifest(
            &ctx.skill_name,
            mode,
            opts.baseline,
            iteration,
            &now_iso8601(),
            &tasks,
        ),
    )?;

    // Write each prompt to its own file; dispatch.json references it by path.
    for task in &tasks {
        fs::write(&task.dispatch_prompt_path, &task.dispatch_prompt)?;
    }

    let dispatch_json_path = iteration_dir.join("dispatch.json");
    let dispatch_json = json!({
        "skill_name": ctx.skill_name,
        "iteration": iteration,
        "run_nonce": run_nonce,
        "iteration_dir": iteration_dir.to_string_lossy(),
        "mode": mode,
        "baseline": opts.baseline,
        "plan_mode": opts.plan_mode,
        "conditions": conditions.conditions,
        "harness": ctx.harness,
        "tasks": tasks,
    });
    write_json(&dispatch_json_path, &dispatch_json)?;

    // Opt-in hard guard: a PreToolUse hook blocking subagent writes/installs
    // outside the eval sandbox while dispatches run.
    if opts.guard && !opts.dry_run {
        if opts.no_stage {
            eprintln!("\n⚠ --guard requires staging enabled; skipping guard install.");
        } else {
            install_guard(
                &ctx.stage_root,
                &ctx.workspace_root,
                &std::env::current_exe()?,
                None,
            )?;
            println!(
                "\n🛡 Write guard armed: a PreToolUse hook is staged in .claude/settings.local.json\n   and will block writes/installs outside the eval sandbox during dispatches.\n   It auto-expires in 6h and is removed on the next run; to remove it now:\n     skill-eval teardown-guard --skill <name>"
            );
        }
    }

    // Plugin-shadow preflight (Claude Code): a staged skill name also discoverable
    // from an enabled plugin or the global skills dir contaminates the run.
    if ctx.harness == Harness::ClaudeCode {
        let mut names: Vec<&str> = vec![ctx.skill_name.as_str()];
        names.extend(ctx.sibling_skill_names.iter().map(String::as_str));
        let report = detect_plugin_shadows(&resolve_config_dir(None), &ctx.stage_root, &names);
        if !report.shadowed.is_empty() {
            write_json(&iteration_dir.join("plugin-shadow.json"), &report)?;
            eprintln!("{}", format_shadow_banner(&report));
        }
    }

    println!("\nWorkspace prepared: {}", iteration_dir.display());
    println!("Dispatch manifest:  {}", manifest_path.display());
    println!("Dispatch tasks:     {}", dispatch_json_path.display());
    println!(
        "\n{} dispatches required ({} evals × 2 conditions).",
        tasks.len(),
        selected_evals.len()
    );

    if opts.dry_run {
        println!("\n--dry-run: stopping after workspace prep.");
    } else if ctx.harness == Harness::Codex {
        println!(
            "\nNext: iterate the tasks[] array in dispatch.json and dispatch each task with codex exec --json, writing each stream to its outputs/codex-events.jsonl. Then run `ingest --iteration {iteration} --harness codex`."
        );
    } else {
        println!(
            "\nNext: iterate the tasks[] array in dispatch.json and dispatch each task as a subagent. Then run:\n  skill-eval ingest --skill {} --skill-dir {} --iteration {iteration} \\\n    --subagents-dir ~/.claude/projects/<project-slug>/<session-id>/subagents/\n(The session ID is the parent session's ID — find it in the Claude Code session URL or from a tool-result path.)",
            ctx.skill_name,
            ctx.skill_dir.display()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warns_for_staged_claude_code_naming_both_paths() {
        let warning = staging_discovery_warning(Harness::ClaudeCode, false).unwrap();
        assert!(warning.contains("fresh"));
        assert!(warning.contains("--no-stage"));
        assert!(warning.contains("detect-stray-writes"));
    }

    #[test]
    fn silent_when_no_stage() {
        assert!(staging_discovery_warning(Harness::ClaudeCode, true).is_none());
    }

    #[test]
    fn silent_for_codex() {
        assert!(staging_discovery_warning(Harness::Codex, false).is_none());
    }

    #[test]
    fn base36_roundtrips_small_values() {
        assert_eq!(to_base36(0), "0");
        assert_eq!(to_base36(35), "z");
        assert_eq!(to_base36(36), "10");
    }

    #[test]
    fn next_iteration_uses_override_then_scans() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(next_iteration(tmp.path(), Some(7)), 7);
        assert_eq!(next_iteration(&tmp.path().join("nope"), None), 1);
        fs::create_dir_all(tmp.path().join("iteration-1")).unwrap();
        fs::create_dir_all(tmp.path().join("iteration-4")).unwrap();
        fs::create_dir_all(tmp.path().join("not-an-iteration")).unwrap();
        assert_eq!(next_iteration(tmp.path(), None), 5);
    }
}
