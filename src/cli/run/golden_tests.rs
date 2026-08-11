//! Golden-fixture tests pinning the generated harness artifacts byte-for-byte.
//!
//! Every fixture under `tests/golden/<harness>/` is rendered from a fixed,
//! deterministic context (no tempdirs, no clocks) so the comparison is exact,
//! guarding the generated artifacts byte-for-byte against descriptor and
//! renderer edits.
//!
//! To regenerate after an intentional output change:
//! `GOLDEN_BLESS=1 cargo test golden_` — then review the fixture diff.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::adapters::{CliDispatchContext, CliJudgeContext, adapter_for};
use crate::core::{AvailableSkill, Harness, Mode};

use super::dispatch::{DispatchTaskOpts, ManifestContext, build_dispatch_task, build_manifest};
use super::runbook::{RunbookContext, build_runbook};

fn empty_env() -> &'static BTreeMap<String, String> {
    static EMPTY: LazyLock<BTreeMap<String, String>> = LazyLock::new(BTreeMap::new);
    &EMPTY
}

fn golden_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(rel)
}

fn assert_golden(rel: &str, actual: &str) {
    let path = golden_path(rel);
    if std::env::var_os("GOLDEN_BLESS").is_some_and(|v| v == "1") {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "golden fixture {} is missing — run `GOLDEN_BLESS=1 cargo test golden_` and commit the fixtures",
            path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "output diverged from tests/golden/{rel}; if the change is intentional, \
         re-bless with `GOLDEN_BLESS=1 cargo test golden_` and review the fixture diff"
    );
}

fn fixed_skills() -> Vec<AvailableSkill> {
    vec![
        AvailableSkill {
            name: "widget-skill".to_string(),
            path: "/work/staged/widget-skill/SKILL.md".to_string(),
            description: "Builds widgets the house way.".to_string(),
        },
        AvailableSkill {
            name: "aux-helper".to_string(),
            path: "/work/staged/aux-helper/SKILL.md".to_string(),
            description: "Assists with auxiliary chores.".to_string(),
        },
    ]
}

fn staged_task(harness: Harness) -> DispatchTaskOpts<'static> {
    DispatchTaskOpts {
        eval_id: "demo-eval",
        condition: "with_skill",
        staged_skill_path: Some("/work/staged/widget-skill/SKILL.md"),
        user_prompt: "Build me a widget.",
        fixtures: vec!["/work/fixtures/input.txt".to_string()],
        outputs_dir: "/work/outputs",
        eval_root: Some("/work/task"),
        cond_dir: "/work/cond",
        bootstrap_content: Some("Session guidelines: be concise."),
        plan_mode_content: Some("PLAN STEP"),
        skill_name: "widget-skill",
        available_skills: fixed_skills(),
        harness,
        ..Default::default()
    }
}

fn bare_task(harness: Harness) -> DispatchTaskOpts<'static> {
    DispatchTaskOpts {
        eval_id: "demo-eval",
        condition: "without_skill",
        user_prompt: "Build me a widget.",
        fixtures: vec!["/work/fixtures/input.txt".to_string()],
        outputs_dir: "/work/outputs-b",
        eval_root: Some("/work/task-b"),
        cond_dir: "/work/cond-b",
        skill_name: "widget-skill",
        harness,
        ..Default::default()
    }
}

fn render_manifest(harness: Harness, guard: bool, agent_model: Option<&str>) -> String {
    let slug =
        adapter_for(harness).staged_slug("slow-powers-eval-", 2, "with_skill", "widget-skill");
    let mut staged = staged_task(harness);
    staged.staged_skill_slug = Some(&slug);
    let tasks = vec![
        build_dispatch_task(&staged).unwrap(),
        build_dispatch_task(&bare_task(harness)).unwrap(),
    ];
    build_manifest(
        "widget-skill",
        Mode::Revision,
        Some("iteration-1"),
        2,
        "2026-01-01T00:00:00Z",
        &tasks,
        ManifestContext {
            harness,
            guard,
            agent_model,
            agent_env: empty_env(),
        },
    )
}

#[test]
fn golden_runbook_per_harness() {
    for harness in Harness::known() {
        let label = adapter_for(harness).label();
        let dir = PathBuf::from("/work/.eval-magic/widget-skill/iteration-2");
        let book = build_runbook(&RunbookContext {
            harness,
            skill_name: "widget-skill",
            iteration: 2,
            iteration_dir: &dir,
            mode: Mode::Revision,
            cond_a: "old_skill",
            cond_b: "new_skill",
            num_tasks: 6,
            multi_turn_tasks: 0,
            target_args: " --skill-dir /tmp/skills --skill widget-skill",
            guard: true,
            agent_model: Some("model-x"),
            agent_env: empty_env(),
        });
        assert_golden(&format!("{label}/runbook.golden.md"), &book);
    }
}

#[test]
fn golden_manifest_per_harness() {
    for harness in Harness::known() {
        let label = adapter_for(harness).label();
        let manifest = render_manifest(harness, true, Some("model-x"));
        assert_golden(&format!("{label}/manifest.golden.md"), &manifest);
    }
}

#[test]
fn golden_manifest_codex_without_guard() {
    // Pins the hook-trust conditional: no --dangerously-bypass-hook-trust.
    let manifest = render_manifest(Harness::resolve("codex").unwrap(), false, Some("model-x"));
    assert_golden("codex/manifest-noguard.golden.md", &manifest);
}

#[test]
fn golden_manifest_claude_without_model() {
    // Pins the empty model-arg rendering.
    let manifest = render_manifest(Harness::resolve("claude-code").unwrap(), true, None);
    assert_golden("claude-code/manifest-nomodel.golden.md", &manifest);
}

#[test]
fn golden_dispatch_prompt_per_harness() {
    for harness in Harness::known() {
        let label = adapter_for(harness).label();
        let slug =
            adapter_for(harness).staged_slug("slow-powers-eval-", 2, "with_skill", "widget-skill");
        let mut opts = staged_task(harness);
        opts.staged_skill_slug = Some(&slug);
        let task = build_dispatch_task(&opts).unwrap();
        assert_golden(
            &format!("{label}/dispatch-prompt.golden.txt"),
            &task.dispatch_prompt,
        );
    }
}

#[test]
fn golden_skills_block_per_harness() {
    for harness in Harness::known() {
        let label = adapter_for(harness).label();
        let block = adapter_for(harness).render_available_skills_block(&fixed_skills());
        assert_golden(&format!("{label}/skills-block.golden.txt"), &block);
    }
}

#[test]
fn golden_guard_armed_message_per_harness() {
    // The post-arm banner printed for every guard-capable harness — the
    // operator-facing contract of the armed guard. Guardless harnesses have
    // no banner to pin.
    for harness in Harness::known() {
        let adapter = adapter_for(harness);
        let label = adapter.label();
        let Some(message) = adapter.guard_armed_message() else {
            continue;
        };
        assert_golden(&format!("{label}/guard-armed.golden.txt"), &message);
    }
}

#[test]
fn golden_judge_recipe_per_harness() {
    for (harness, guard, rel) in [
        (
            Harness::resolve("claude-code").unwrap(),
            true,
            "claude-code/judge-recipe.golden.md",
        ),
        (
            Harness::resolve("codex").unwrap(),
            true,
            "codex/judge-recipe.golden.md",
        ),
        // Cline has no guard, so one variant covers both guard states.
        (
            Harness::resolve("cline").unwrap(),
            false,
            "cline/judge-recipe.golden.md",
        ),
        // Pins the hook-trust conditional in the judge command line.
        (
            Harness::resolve("codex").unwrap(),
            false,
            "codex/judge-recipe-noguard.golden.md",
        ),
        // OpenCode has no guard args, so one variant covers both guard states.
        (
            Harness::resolve("opencode").unwrap(),
            false,
            "opencode/judge-recipe.golden.md",
        ),
    ] {
        let recipe = adapter_for(harness)
            .cli_judge_next_steps(CliJudgeContext {
                guard,
                iteration_dir: Path::new("/work/iter-1"),
            })
            .expect("judge recipe is wired for this harness");
        assert_golden(rel, &recipe);
    }
}

#[test]
fn golden_cline_next_steps_with_and_without_model() {
    for (agent_model, rel) in [
        (Some("model-x"), "cline/next-steps-model.golden.txt"),
        (None, "cline/next-steps-nomodel.golden.txt"),
    ] {
        let steps =
            adapter_for(Harness::resolve("cline").unwrap()).cli_next_steps(CliDispatchContext {
                guard: false,
                target_args: " --skill-dir /tmp/skills --skill widget-skill",
                iteration: 2,
                agent_model,
                agent_env: empty_env(),
            });
        assert_golden(rel, &steps);
    }
}

#[test]
fn golden_opencode_next_steps_with_and_without_model() {
    for (agent_model, rel) in [
        (Some("model-x"), "opencode/next-steps-model.golden.txt"),
        (None, "opencode/next-steps-nomodel.golden.txt"),
    ] {
        let steps =
            adapter_for(Harness::resolve("opencode").unwrap()).cli_next_steps(CliDispatchContext {
                guard: false,
                target_args: " --skill-dir /tmp/skills --skill widget-skill",
                iteration: 2,
                agent_model,
                agent_env: empty_env(),
            });
        assert_golden(rel, &steps);
    }
}
