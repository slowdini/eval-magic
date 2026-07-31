//! Shared rendering helpers for harness CLI command templates
//! (Codex's `codex exec`, Claude Code's `claude -p`).

use std::collections::BTreeMap;

use crate::core::GIT_ROUTING_ENV_VARS;

/// POSIX-shell prelude used for every eval-agent process. Task cwd is the
/// repository boundary; inherited routing variables must not override it.
pub(crate) fn git_environment_prelude() -> String {
    format!("unset {}", GIT_ROUTING_ENV_VARS.join(" "))
}

/// Prefix one one-shot or resumed eval-agent command with the Git sanitation
/// prelude. Configured exports are scoped to a subshell so a copied recipe
/// cannot leak them into later judge or runner commands.
pub(crate) fn render_agent_dispatch_command(
    command: &str,
    environment: &BTreeMap<String, String>,
) -> String {
    if environment.is_empty() {
        return format!("{}\n{command}", git_environment_prelude());
    }

    let mut lines = vec!["(".to_string(), git_environment_prelude()];
    lines.extend(
        environment
            .iter()
            .map(|(name, value)| format!("export {name}={}", shell_quote_arg(value))),
    );
    lines.push(command.to_string());
    lines.push(")".to_string());
    lines.join("\n")
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
///
/// The separators come from `tr`, not from a `\u0000` escape inside the jq
/// program: an escape only works if it reaches jq unresolved, and a tool that
/// materialises this recipe writes real NUL bytes into the program text
/// instead, where they do not survive argument passing — jq then emits no
/// separators and `xargs -0` collapses every field into one bogus dispatch
/// that exits 0. Paths containing a newline remain unsupported, as before.
pub(crate) fn render_parallel_dispatch_recipe(
    command_block: &str,
    one_shot_only: bool,
    environment: &BTreeMap<String, String>,
) -> String {
    let tasks = if one_shot_only {
        ".tasks[] | select(.turns == null)"
    } else {
        ".tasks[]"
    };
    let mut lines = vec![
        "JOBS=${JOBS:-4}".to_string(),
        format!(
            "jq -r '{tasks} | .eval_root, .dispatch_prompt_path, .outputs_dir' dispatch.json \\"
        ),
        "  | tr '\\n' '\\0' \\".to_string(),
        "  | xargs -0 -P \"$JOBS\" -n 3 sh -c '".to_string(),
        "    eval_root=\"$1\"".to_string(),
        "    prompt_path=\"$2\"".to_string(),
        "    outputs_dir=\"$3\"".to_string(),
        "    mkdir -p \"$outputs_dir\"".to_string(),
        format!("    {}", git_environment_prelude()),
    ];
    lines.extend(
        environment
            .iter()
            .map(|(name, value)| format!("    export {name}={}", shell_quote_arg(value))),
    );
    lines.extend([command_block.to_string(), "  ' sh".to_string()]);
    lines.join("\n")
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
        "jq -r '.tasks[] | .dispatch_prompt_path, .response_path, (\"model=\" + (.model // \"\"))' judge-tasks.json \\".to_string(),
        "  | tr '\\n' '\\0' \\".to_string(),
        "  | xargs -0 -P \"$JOBS\" -n 3 sh -c '".to_string(),
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
    use std::collections::BTreeMap;

    use super::{
        render_agent_dispatch_command, render_cli_model_arg, render_judge_dispatch_recipe,
        render_parallel_dispatch_recipe, shell_quote_arg,
    };

    #[test]
    fn agent_dispatch_environment_is_sorted_and_shell_quoted() {
        let env = BTreeMap::from([
            ("QUOTE".to_string(), "a'b".to_string()),
            ("EMPTY".to_string(), String::new()),
            ("MODE".to_string(), "strict mode".to_string()),
        ]);

        let rendered = render_agent_dispatch_command("agent run", &env);

        assert!(rendered.starts_with("(\nunset GIT_DIR GIT_WORK_TREE"));
        assert!(rendered.ends_with("agent run\n)"));
        assert!(
            rendered.contains(
                "export EMPTY=\nexport MODE='strict mode'\nexport QUOTE='a'\"'\"'b'\nagent run"
            ),
            "{rendered}"
        );
    }

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
        let recipe =
            render_parallel_dispatch_recipe("    run \"$eval_root\"", false, &BTreeMap::new());

        assert!(recipe.contains(
            "jq -r '.tasks[] | .eval_root, .dispatch_prompt_path, .outputs_dir' dispatch.json"
        ));
        assert!(recipe.contains("| tr '\\n' '\\0' \\"), "{recipe}");
        assert!(
            recipe.contains("| xargs -0 -P \"$JOBS\" -n 3 sh -c '"),
            "{recipe}"
        );
        assert!(recipe.contains("eval_root=\"$1\""), "{recipe}");
        assert!(recipe.contains("prompt_path=\"$2\""), "{recipe}");
        assert!(recipe.contains("outputs_dir=\"$3\""), "{recipe}");
        assert!(!recipe.contains("-I{}"), "{recipe}");
        assert!(!recipe.contains("cut -f"), "{recipe}");
    }

    /// The separators must be produced by `tr`, never spelled as an escape
    /// inside the jq program: a tool that materialises the recipe and resolves
    /// `\u0000` writes real NUL bytes into the program text, where they do not
    /// survive argument passing. jq then emits no separators, `xargs -0`
    /// collapses every field into one argument, and the result is a single
    /// bogus dispatch that exits 0 without dispatching anything.
    #[test]
    fn recipes_carry_no_nul_escape_for_a_materialiser_to_resolve() {
        for recipe in [
            render_parallel_dispatch_recipe("    run \"$eval_root\"", false, &BTreeMap::new()),
            render_parallel_dispatch_recipe("    run \"$eval_root\"", true, &BTreeMap::new()),
            render_judge_dispatch_recipe("    judge $model_arg \\", "--model", "judge"),
        ] {
            assert!(!recipe.contains("\\u0000"), "{recipe}");
            assert!(!recipe.contains('\0'), "{recipe}");
        }
    }

    #[test]
    fn judge_recipe_preserves_an_empty_model_and_skips_existing_responses() {
        let recipe = render_judge_dispatch_recipe("    judge $model_arg \\", "--model", "judge");

        assert!(recipe.contains(
            "jq -r '.tasks[] | .dispatch_prompt_path, .response_path, (\"model=\" + (.model // \"\"))' judge-tasks.json"
        ));
        assert!(recipe.contains("| tr '\\n' '\\0' \\"), "{recipe}");
        assert!(
            recipe.contains("| xargs -0 -P \"$JOBS\" -n 3 sh -c '"),
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
