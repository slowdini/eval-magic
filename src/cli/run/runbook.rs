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

use std::path::Path;

use crate::adapters::RUNBOOK_TEMPLATE;
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
    /// The self-sufficient `--skill-dir … --skill …` selector (leading space),
    /// from [`command_target_args`](crate::cli::command_target_args).
    pub target_args: &'a str,
}

/// Render `RUNBOOK.md` for a run: fill the shared runbook template's
/// `{{TOKEN}}` placeholders with run-specific values. The harness-specific
/// dispatch + judge recipes come from the adapter's CLI generators, so the
/// runbook stays in lockstep with `dispatch-manifest.md` and the printed next
/// steps; pipeline commands carry `--harness`.
pub(crate) fn build_runbook(ctx: &RunbookContext) -> String {
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

    // One command per phase: the runner drives every dispatch, so nothing here
    // varies by harness beyond the `--harness` selector itself.
    let label = harness_label(ctx.harness);
    let dispatch_cmd = format!(
        "eval-magic dispatch{} --iteration {} --harness {label}",
        ctx.target_args, ctx.iteration
    );
    let ingest_cmd = format!(
        "eval-magic ingest{} --iteration {} --harness {label}",
        ctx.target_args, ctx.iteration
    );
    let judge_cmd = format!(
        "eval-magic dispatch --judges{} --iteration {} --harness {label}",
        ctx.target_args, ctx.iteration
    );
    let finalize_cmd = format!(
        "eval-magic finalize{} --iteration {} --harness {label}",
        ctx.target_args, ctx.iteration
    );
    let teardown_cmd = format!("eval-magic teardown{} --harness {label}", ctx.target_args);
    vars.push(("HARNESS", &label));
    vars.push(("DISPATCH_CMD", &dispatch_cmd));
    vars.push(("INGEST_CMD", &ingest_cmd));
    vars.push(("JUDGE_CMD", &judge_cmd));
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
            target_args: " --skill-dir /tmp/skills --skill widget-skill",
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

        // Every phase is a runner command carrying --harness codex, so the whole
        // runbook is copy-pasteable without knowing the harness's own CLI.
        assert!(
            book.contains("eval-magic dispatch --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness codex"),
            "carries the dispatch command: {book}"
        );
        assert!(
            book.contains("eval-magic dispatch --judges --skill-dir /tmp/skills --skill widget-skill --iteration 2 --harness codex"),
            "carries the judge dispatch command: {book}"
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

    /// The runbook reads the same whether or not the plan holds scripted turns:
    /// the runner drives both, so there is nothing to branch on.
    #[test]
    fn a_scripted_plan_reads_the_same_as_a_one_shot_plan() {
        let dir = PathBuf::from("/work/.eval-magic/widget-skill/iteration-2");
        let context = |num_tasks: usize| RunbookContext {
            harness: Harness::resolve("codex").unwrap(),
            skill_name: "widget-skill",
            iteration: 2,
            iteration_dir: &dir,
            mode: Mode::NewSkill,
            cond_a: "with_skill",
            cond_b: "without_skill",
            num_tasks,
            target_args: " --skill /tmp/widget-skill",
        };
        let book = build_runbook(&context(4));
        assert!(book.contains("eval-magic dispatch --skill /tmp/widget-skill"));
        assert!(book.contains("conversation.json"));
        assert!(
            !book.contains("--output-last-message <outputs_dir>/final-message.md"),
            "the runbook no longer carries a harness CLI recipe: {book}"
        );
        // Only the dispatch count differs between plans.
        assert_eq!(
            build_runbook(&context(4)).replace("**Dispatches:** 4", "**Dispatches:** 6"),
            build_runbook(&context(6))
        );
    }
}
