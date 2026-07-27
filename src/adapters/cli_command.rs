//! Shared rendering helpers for harness CLI command templates
//! (Codex's `codex exec`, Claude Code's `claude -p`).

use crate::core::GIT_ROUTING_ENV_VARS;

/// POSIX-shell prelude used for every eval-agent process. Task cwd is the
/// repository boundary; inherited routing variables must not override it.
pub(crate) fn git_environment_prelude() -> String {
    format!("unset {}", GIT_ROUTING_ENV_VARS.join(" "))
}

/// Prefix one one-shot or resumed eval-agent command with the Git sanitation
/// prelude. Judge commands intentionally use their separate recipe.
pub(crate) fn render_agent_dispatch_command(command: &str) -> String {
    format!("{}\n{command}", git_environment_prelude())
}

/// Quote a value for a POSIX shell only when it contains anything outside a
/// conservative safe set, single-quoting and escaping embedded quotes otherwise.
pub(crate) fn shell_quote_arg(value: &str) -> String {
    if value.bytes().all(|b| {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b':' | b'@' | b'+')
    }) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Render a ` <flag> <model>` fragment for a CLI dispatch, or an empty string
/// when the adapter has no model flag or no (non-blank) model was declared.
pub(crate) fn render_cli_model_arg(flag: Option<&str>, model: Option<&str>) -> String {
    let Some(model) = model.filter(|m| !m.trim().is_empty()) else {
        return String::new();
    };
    let Some(flag) = flag else {
        return String::new();
    };
    format!(" {flag} {}", shell_quote_arg(model))
}

/// Render the shared parallel-dispatch recipe: the jq/xargs scaffold over
/// `dispatch.json` tasks with the harness's per-task command block spliced in.
///
/// `command_block` is the (possibly multi-line) command run per task inside
/// the `sh -c` body; it references `$eval_root` / `$prompt_path` /
/// `$outputs_dir`. The three values travel as separate NUL-delimited arguments
/// so BSD xargs does not apply its 255-byte `-I` replacement limit.
pub(crate) fn render_parallel_dispatch_recipe(command_block: &str, one_shot_only: bool) -> String {
    let tasks = if one_shot_only {
        ".tasks[] | select(.turns == null)"
    } else {
        ".tasks[]"
    };
    [
        "JOBS=${JOBS:-4}".to_string(),
        format!(
            "jq -j '{tasks} | .eval_root, \"\\u0000\", .dispatch_prompt_path, \"\\u0000\", \
             .outputs_dir, \"\\u0000\"' dispatch.json | \\"
        ),
        "  xargs -0 -P \"$JOBS\" -n 3 sh -c '".to_string(),
        "    eval_root=\"$1\"".to_string(),
        "    prompt_path=\"$2\"".to_string(),
        "    outputs_dir=\"$3\"".to_string(),
        "    mkdir -p \"$outputs_dir\"".to_string(),
        format!("    {}", git_environment_prelude()),
        command_block.to_string(),
        "  ' sh".to_string(),
    ]
    .join("\n")
}

/// Render the shared judge-dispatch recipe: the jq/xargs scaffold over
/// `judge-tasks.json` with the harness command line spliced in.
///
/// `command_line` must reference `$model_arg` (empty when the task declares no
/// model, `<flag> <model>` otherwise) and end with ` \`; `model_flag` fills the
/// `model_arg` assignment; `capture_prefix` names the per-task
/// `$response_base.<prefix>-events.jsonl` / `.<prefix>-stderr.log` captures.
pub(crate) fn render_judge_dispatch_recipe(
    command_line: &str,
    model_flag: &str,
    capture_prefix: &str,
) -> String {
    [
        "Dispatch each judge task from judge-tasks.json with:".to_string(),
        "Existing nonempty response files are skipped; delete one to dispatch that judge again."
            .to_string(),
        String::new(),
        "```bash".to_string(),
        "JOBS=${JOBS:-4}".to_string(),
        "jq -j '.tasks[] | .dispatch_prompt_path, \"\\u0000\", .response_path, \"\\u0000\", (\"model=\" + (.model // \"\")), \"\\u0000\"' judge-tasks.json | \\".to_string(),
        "  xargs -0 -P \"$JOBS\" -n 3 sh -c '".to_string(),
        "    prompt_path=\"$1\"".to_string(),
        "    response_path=\"$2\"".to_string(),
        "    model=\"${3#model=}\"".to_string(),
        "    if [ -s \"$response_path\" ]; then exit 0; fi".to_string(),
        "    response_base=\"${response_path%.json}\"".to_string(),
        "    mkdir -p \"$(dirname \"$response_path\")\"".to_string(),
        format!("    model_arg=\"\"; [ -n \"$model\" ] && model_arg=\"{model_flag} $model\""),
        command_line.to_string(),
        "      \"Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers.\" \\".to_string(),
        "      </dev/null \\".to_string(),
        format!("      > \"$response_base.{capture_prefix}-events.jsonl\" \\"),
        format!("      2> \"$response_base.{capture_prefix}-stderr.log\""),
        "  ' sh".to_string(),
        "```".to_string(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        render_cli_model_arg, render_judge_dispatch_recipe, render_parallel_dispatch_recipe,
        shell_quote_arg,
    };

    #[test]
    fn shell_quote_leaves_safe_values_unquoted() {
        assert_eq!(shell_quote_arg("gpt-5-mini"), "gpt-5-mini");
        assert_eq!(shell_quote_arg("claude-opus-4-8"), "claude-opus-4-8");
        assert_eq!(shell_quote_arg("a/b:c@d+e_f.g"), "a/b:c@d+e_f.g");
    }

    #[test]
    fn shell_quote_wraps_values_with_specials() {
        assert_eq!(shell_quote_arg("a b"), "'a b'");
        assert_eq!(shell_quote_arg("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn render_model_arg_empty_when_unset() {
        assert_eq!(render_cli_model_arg(Some("--model"), None), "");
        assert_eq!(render_cli_model_arg(Some("--model"), Some("   ")), "");
        assert_eq!(render_cli_model_arg(None, Some("opus")), "");
    }

    #[test]
    fn render_model_arg_renders_flag_and_quoted_model() {
        assert_eq!(
            render_cli_model_arg(Some("--model"), Some("opus")),
            " --model opus"
        );
        assert_eq!(
            render_cli_model_arg(Some("-m"), Some("gpt 5")),
            " -m 'gpt 5'"
        );
    }

    #[test]
    fn parallel_recipe_batches_three_nul_delimited_arguments_without_replacement() {
        let recipe = render_parallel_dispatch_recipe("    run \"$eval_root\"", false);

        assert!(recipe.contains(
            "jq -j '.tasks[] | .eval_root, \"\\u0000\", .dispatch_prompt_path, \"\\u0000\", .outputs_dir, \"\\u0000\"' dispatch.json"
        ));
        assert!(
            recipe.contains("xargs -0 -P \"$JOBS\" -n 3 sh -c '"),
            "{recipe}"
        );
        assert!(recipe.contains("eval_root=\"$1\""), "{recipe}");
        assert!(recipe.contains("prompt_path=\"$2\""), "{recipe}");
        assert!(recipe.contains("outputs_dir=\"$3\""), "{recipe}");
        assert!(!recipe.contains("-I{}"), "{recipe}");
        assert!(!recipe.contains("cut -f"), "{recipe}");
    }

    #[test]
    fn judge_recipe_preserves_an_empty_model_and_skips_existing_responses() {
        let recipe = render_judge_dispatch_recipe("    judge $model_arg \\", "--model", "judge");

        assert!(recipe.contains(
            "jq -j '.tasks[] | .dispatch_prompt_path, \"\\u0000\", .response_path, \"\\u0000\", (\"model=\" + (.model // \"\")), \"\\u0000\"' judge-tasks.json"
        ));
        assert!(
            recipe.contains("xargs -0 -P \"$JOBS\" -n 3 sh -c '"),
            "{recipe}"
        );
        assert!(recipe.contains("prompt_path=\"$1\""), "{recipe}");
        assert!(recipe.contains("response_path=\"$2\""), "{recipe}");
        assert!(recipe.contains("model=\"${3#model=}\""), "{recipe}");
        assert!(
            recipe.contains("if [ -s \"$response_path\" ]; then exit 0; fi"),
            "{recipe}"
        );
        assert!(
            recipe.contains(
                "Existing nonempty response files are skipped; delete one to dispatch that judge again."
            ),
            "{recipe}"
        );
        assert!(!recipe.contains("-I{}"), "{recipe}");
        assert!(!recipe.contains("cut -f"), "{recipe}");
    }
}
