//! `RUNBOOK.md` generation — the followable handoff artifact written into an
//! iteration directory during `run`.
//!
//! The runbook turns the prep session's "what to do next" guidance into a file
//! a human at a terminal can read end-to-end: "Read and follow RUNBOOK.md". Every
//! run uses the shared [`RUNBOOK_TEMPLATE`], whose harness-specific dispatch +
//! judge recipes come from the adapter's CLI generators.
//!
//! The prose skeleton lives in `profiles/` (checked in) and carries `{{TOKEN}}`
//! placeholders the renderer fills with run-specific values. The generated
//! `RUNBOOK.md` itself is a workspace artifact and is not version controlled.

use std::collections::BTreeMap;
use std::path::Path;

use crate::adapters::{CliDispatchContext, CliJudgeContext, RUNBOOK_TEMPLATE, adapter_for};
use crate::core::fs::artifact_path;
use crate::core::{Harness, Mode, POSIX_TOOLING_REQUIREMENT};

use super::util::{harness_label, mode_str};

/// Run-specific values the renderer substitutes into a runbook template. Built by
/// the orchestrator from the resolved run; kept as primitives so the renderer is
/// decoupled from the orchestrator's private `Resolved`/`RunContext` types and is
/// unit-testable on its own.
pub(crate) struct RunbookContext<'a> {
    pub harness: Harness,
    pub skill_name: &'a str,
    pub iteration: u32,
    pub iteration_dir: &'a Path,
    pub mode: Mode,
    pub cond_a: &'a str,
    pub cond_b: &'a str,
    pub num_tasks: usize,
    pub multi_turn_tasks: usize,
    /// The self-sufficient `--skill-dir … --skill …` selector (leading space),
    /// from [`command_target_args`](crate::cli::command_target_args).
    pub target_args: &'a str,
    pub guard: bool,
    pub agent_model: Option<&'a str>,
    pub agent_env: &'a BTreeMap<String, String>,
}

/// Render `RUNBOOK.md` for a run: fill the shared runbook template's
/// `{{TOKEN}}` placeholders with run-specific values. The harness-specific
/// dispatch + judge recipes come from the adapter's CLI generators, so the
/// runbook stays in lockstep with `dispatch-manifest.md` and the printed next
/// steps; pipeline commands carry `--harness`.
pub(crate) fn build_runbook(ctx: &RunbookContext) -> String {
    let adapter = adapter_for(ctx.harness);
    let template = RUNBOOK_TEMPLATE;

    let iteration = ctx.iteration.to_string();
    let num_tasks = ctx.num_tasks.to_string();
    let dispatch_json = artifact_path(&ctx.iteration_dir.join("dispatch.json"));
    let benchmark_path = artifact_path(&ctx.iteration_dir.join("benchmark.json"));

    // Shared identity tokens, present in both templates.
    let mut vars: Vec<(&str, &str)> = vec![
        ("SKILL_NAME", ctx.skill_name),
        ("ITERATION", &iteration),
        ("MODE", mode_str(ctx.mode)),
        ("COND_A", ctx.cond_a),
        ("COND_B", ctx.cond_b),
        ("NUM_TASKS", &num_tasks),
        ("DISPATCH_JSON", &dispatch_json),
        ("BENCHMARK_PATH", &benchmark_path),
        // Every recipe below this line is a POSIX command line, so the runbook
        // names the shell it expects before the reader reaches one (issue #248).
        ("POSIX_REQUIREMENT", POSIX_TOOLING_REQUIREMENT),
    ];

    // A human pastes commands. The harness-specific dispatch + judge recipes come
    // from the adapter's CLI generators, so the runbook stays in lockstep with
    // `dispatch-manifest.md` and the printed next steps; pipeline commands carry
    // `--harness`. Owners outlive the `render` call below.
    let label = harness_label(ctx.harness);
    let one_shot_recipe = adapter.cli_next_steps(CliDispatchContext {
        guard: ctx.guard,
        target_args: ctx.target_args,
        iteration: ctx.iteration,
        agent_model: ctx.agent_model,
        agent_env: ctx.agent_env,
    });
    let dispatch_recipe = if ctx.multi_turn_tasks == 0 {
        one_shot_recipe
    } else {
        let driver = format!(
            "Scripted tasks must run through eval-magic's conversation driver so every follow-up \
             resumes the same native session and produces a schema-validated \
             `conversation.json`. From this iteration directory:\n\n```bash\n\
             JOBS=${{JOBS:-4}}\n\
             jq -r '.tasks | to_entries[] | select(.value.turns != null) | .key' \
             \"{dispatch_json}\" | \\\n  xargs -P \"$JOBS\" -n 1 eval-magic dispatch-task \
             --dispatch \"{dispatch_json}\" --task-index\n```\n\n\
             A normal guardrail stop (`agent_did_not_ask` or `agent_response_mismatch`) is valid \
             completed eval data; an interrupted task has no `conversation.json` and ingest \
             skips it."
        );
        if ctx.multi_turn_tasks == ctx.num_tasks {
            format!(
                "{driver}\n\nThen run `eval-magic ingest{} --iteration {} --harness {label}`.",
                ctx.target_args, ctx.iteration
            )
        } else {
            format!(
                "{driver}\n\nFor the remaining task entries whose `turns` field is absent, use \
                 the one-shot harness recipe below (do not use it for scripted tasks):\n\
                 {one_shot_recipe}"
            )
        }
    };
    let judge_recipe = adapter
        .cli_judge_next_steps(CliJudgeContext {
            guard: ctx.guard,
            iteration_dir: ctx.iteration_dir,
        })
        .unwrap_or_else(|| {
            "Dispatch each judge task `ingest` listed through the same harness CLI, \
             capturing its transcript output, then finalize."
                .to_string()
        });
    let finalize_cmd = format!(
        "eval-magic finalize{} --iteration {} --harness {label}",
        ctx.target_args, ctx.iteration
    );
    let teardown_cmd = format!("eval-magic teardown{} --harness {label}", ctx.target_args);
    vars.push(("HARNESS", &label));
    vars.push(("DISPATCH_RECIPE", &dispatch_recipe));
    vars.push(("JUDGE_RECIPE", &judge_recipe));
    vars.push(("FINALIZE_CMD", &finalize_cmd));
    vars.push(("TEARDOWN_CMD", &teardown_cmd));

    render(template, &vars)
}

/// Substitute `{{KEY}}` placeholders in `template` with their values.
///
/// Each `(key, value)` replaces every `{{key}}` occurrence. Keys are matched
/// verbatim (the braces are added here), so callers pass `"SKILL_NAME"`, not
/// `"{{SKILL_NAME}}"`. Replacement is a single ordered pass per key, so a value
/// that itself contains `{{...}}` is never re-expanded.
fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    // Single left-to-right pass: only the original template is scanned, so a
    // substituted value that itself contains `{{...}}` is emitted verbatim and
    // never re-expanded (order-independent). Unknown / unterminated tokens are
    // left as-is.
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str("{{");
            rest = after;
            continue;
        };
        let key = &after[..end];
        match vars.iter().find(|(k, _)| *k == key) {
            Some((_, value)) => out.push_str(value),
            None => {
                out.push_str("{{");
                out.push_str(key);
                out.push_str("}}");
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::LazyLock;

    fn empty_env() -> &'static BTreeMap<String, String> {
        static EMPTY: LazyLock<BTreeMap<String, String>> = LazyLock::new(BTreeMap::new);
        &EMPTY
    }

    #[test]
    fn runbook_is_human_followed_cli_recipe() {
        let dir = PathBuf::from("/work/.eval-magic/widget-skill/iteration-2");
        let ctx = RunbookContext {
            harness: Harness::resolve("codex").unwrap(),
            skill_name: "widget-skill",
            iteration: 2,
            iteration_dir: &dir,
            mode: Mode::Revision,
            cond_a: "old_skill",
            cond_b: "new_skill",
            num_tasks: 6,
            multi_turn_tasks: 0,
            target_args: " --skill-dir /tmp/skills --skill widget-skill",
            guard: false,
            agent_model: Some("gpt-5-mini"),
            agent_env: empty_env(),
        };
        let book = build_runbook(&ctx);

        // Run-specific identity, including the revision-mode condition names.
        assert!(book.contains("widget-skill"), "names the skill: {book}");
        assert!(book.contains("iteration 2"), "names the iteration: {book}");
        assert!(
            book.contains("old_skill") && book.contains("new_skill"),
            "names both conditions: {book}"
        );

        // Human-followed framing (the shared runbook template).
        assert!(
            book.contains("human driving"),
            "frames the run for a human at a terminal: {book}"
        );

        // The CLI dispatch recipe comes from the Codex adapter; pipeline commands
        // carry --harness codex so they are copy-pasteable.
        assert!(
            book.contains("codex --ask-for-approval never exec"),
            "carries the Codex CLI dispatch recipe: {book}"
        );
        assert!(
            book.contains("eval-magic finalize --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness codex"),
            "finalize carries --harness codex: {book}"
        );
        assert!(
            book.contains(
                "eval-magic teardown --skill-dir /tmp/skills --skill widget-skill --harness codex"
            ),
            "teardown carries --harness codex: {book}"
        );
        assert!(
            book.contains("benchmark.json"),
            "points at the result: {book}"
        );
        assert!(
            !book.contains("{{"),
            "no unsubstituted tokens remain: {book}"
        );
    }

    #[test]
    fn render_substitutes_each_token_everywhere() {
        let out = render(
            "skill {{SKILL_NAME}} iteration {{ITERATION}} — run {{SKILL_NAME}} now",
            &[("SKILL_NAME", "my-skill"), ("ITERATION", "3")],
        );
        assert_eq!(out, "skill my-skill iteration 3 — run my-skill now");
    }

    #[test]
    fn render_leaves_unknown_tokens_untouched() {
        let out = render("{{KNOWN}} {{UNKNOWN}}", &[("KNOWN", "ok")]);
        assert_eq!(out, "ok {{UNKNOWN}}");
    }

    #[test]
    fn render_does_not_re_expand_a_substituted_value() {
        // A value that happens to contain a token must not be expanded by a
        // later (key, value) pair — each key gets exactly one pass.
        let out = render(
            "{{A}} {{B}}",
            &[("A", "value-with-{{B}}-inside"), ("B", "second")],
        );
        assert_eq!(out, "value-with-{{B}}-inside second");
    }

    #[test]
    fn all_scripted_runbook_uses_dispatch_task_instead_of_one_shot_recipe() {
        let dir = PathBuf::from("/work/.eval-magic/widget-skill/iteration-2");
        let book = build_runbook(&RunbookContext {
            harness: Harness::resolve("codex").unwrap(),
            skill_name: "widget-skill",
            iteration: 2,
            iteration_dir: &dir,
            mode: Mode::NewSkill,
            cond_a: "with_skill",
            cond_b: "without_skill",
            num_tasks: 4,
            multi_turn_tasks: 4,
            target_args: " --skill /tmp/widget-skill",
            guard: false,
            agent_model: None,
            agent_env: empty_env(),
        });
        assert!(book.contains("eval-magic dispatch-task"));
        assert!(book.contains("conversation.json"));
        assert!(
            !book.contains("--output-last-message <outputs_dir>/final-message.md"),
            "all-scripted runs must not advertise the one-shot eval command"
        );
    }
}
