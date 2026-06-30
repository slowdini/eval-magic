//! `RUNBOOK.md` generation — the followable handoff artifact written into an
//! iteration directory during `run`.
//!
//! The runbook turns the prep session's "what to do next" guidance into a file
//! a human at a terminal can read end-to-end: "Read and follow RUNBOOK.md". Every
//! run uses the shared [`HEADLESS_RUNBOOK_TEMPLATE`], whose harness-specific
//! dispatch + judge recipes come from the adapter's CLI generators.
//!
//! The prose skeleton lives in `profiles/` (checked in) and carries `{{TOKEN}}`
//! placeholders the renderer fills with run-specific values. The generated
//! `RUNBOOK.md` itself is a workspace artifact and is not version controlled.

use std::path::Path;

use crate::adapters::{
    CliDispatchContext, CliJudgeContext, HEADLESS_RUNBOOK_TEMPLATE, adapter_for,
};
use crate::core::{Harness, Mode};

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
    /// The self-sufficient `--skill-dir … --skill …` selector (leading space),
    /// from [`command_target_args`](crate::cli::command_target_args).
    pub target_args: &'a str,
    pub guard: bool,
    pub agent_model: Option<&'a str>,
}

/// Render `RUNBOOK.md` for a run: fill the shared headless template's
/// `{{TOKEN}}` placeholders with run-specific values. The harness-specific
/// dispatch + judge recipes come from the adapter's CLI generators, so the
/// runbook stays in lockstep with `dispatch-manifest.md` and the printed next
/// steps; pipeline commands carry `--harness`.
pub(crate) fn build_runbook(ctx: &RunbookContext) -> String {
    let adapter = adapter_for(ctx.harness);
    let template = HEADLESS_RUNBOOK_TEMPLATE;

    let iteration = ctx.iteration.to_string();
    let num_tasks = ctx.num_tasks.to_string();
    let dispatch_json = ctx
        .iteration_dir
        .join("dispatch.json")
        .display()
        .to_string();
    let benchmark_path = ctx
        .iteration_dir
        .join("benchmark.json")
        .display()
        .to_string();

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
    ];

    // A human pastes commands. The harness-specific dispatch + judge recipes come
    // from the adapter's CLI generators, so the runbook stays in lockstep with
    // `dispatch-manifest.md` and the printed next steps; pipeline commands carry
    // `--harness`. Owners outlive the `render` call below.
    let label = harness_label(ctx.harness);
    let dispatch_recipe = adapter.cli_next_steps(CliDispatchContext {
        guard: ctx.guard,
        target_args: ctx.target_args,
        iteration: ctx.iteration,
        agent_model: ctx.agent_model,
    });
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
    vars.push(("HARNESS", label));
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

    #[test]
    fn headless_runbook_is_human_followed_cli_recipe() {
        let dir = PathBuf::from("/work/.eval-magic/widget-skill/iteration-2");
        let ctx = RunbookContext {
            harness: Harness::Codex,
            skill_name: "widget-skill",
            iteration: 2,
            iteration_dir: &dir,
            mode: Mode::Revision,
            cond_a: "old_skill",
            cond_b: "new_skill",
            num_tasks: 6,
            target_args: " --skill-dir /tmp/skills --skill widget-skill",
            guard: false,
            agent_model: Some("gpt-5-mini"),
        };
        let book = build_runbook(&ctx);

        // Run-specific identity, including the revision-mode condition names.
        assert!(book.contains("widget-skill"), "names the skill: {book}");
        assert!(book.contains("iteration 2"), "names the iteration: {book}");
        assert!(
            book.contains("old_skill") && book.contains("new_skill"),
            "names both conditions: {book}"
        );

        // Human-followed framing (the shared headless template).
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
}
