//! Claude Code `claude -p` command rendering for `DispatchMechanism::Cli`
//! guidance (hybrid / headless run modes).
//!
//! Differences from the Codex recipe, all forced by the `claude` CLI:
//! `--output-format stream-json` requires `--verbose` in `-p` mode; there is no
//! `--cd` flag, so the dispatch runs from the env dir (`cd <eval-root> &&`);
//! and there is no `--output-last-message`, so the final message is recovered
//! from the stream-json `result` event by the transcript adapter rather than
//! written to a file. `</dev/null` detaches stdin so a permission prompt cannot
//! block on a TTY and piped task data cannot become extra prompt context.

use super::cli_command::render_cli_model_arg;

/// Copy/pasteable Claude Code dispatch command template.
pub(crate) fn claude_exec_command_template(
    model_flag: Option<&str>,
    agent_model: Option<&str>,
) -> String {
    let model_arg = render_cli_model_arg(model_flag, agent_model);
    [
        format!(
            "cd <eval-root> && claude -p --output-format stream-json --verbose --permission-mode acceptEdits{model_arg} \\"
        ),
        "  \"Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response your closing summary.\" \\".to_string(),
        "  </dev/null \\".to_string(),
        "  > <outputs_dir>/claude-events.jsonl \\".to_string(),
        "  2> <outputs_dir>/claude-stderr.log".to_string(),
    ]
    .join("\n")
}

/// Parallel dispatch recipe over `dispatch.json` tasks, one `claude -p` per task.
pub(crate) fn claude_parallel_dispatch_recipe(
    model_flag: Option<&str>,
    agent_model: Option<&str>,
) -> String {
    let model_arg = render_cli_model_arg(model_flag, agent_model);
    [
        "JOBS=${JOBS:-4}".to_string(),
        "jq -j '.tasks[] | [.dispatch_prompt_path, .outputs_dir] | @tsv + \"\\u0000\"' dispatch.json | \\".to_string(),
        "  xargs -0 -P \"$JOBS\" -I{} sh -c '".to_string(),
        "    prompt_path=\"$(printf \"%s\" \"$1\" | cut -f1)\"".to_string(),
        "    outputs_dir=\"$(printf \"%s\" \"$1\" | cut -f2)\"".to_string(),
        "    mkdir -p \"$outputs_dir\"".to_string(),
        format!(
            "    cd <eval-root> && claude -p --output-format stream-json --verbose --permission-mode acceptEdits{model_arg} \\"
        ),
        "      \"Read the file at $prompt_path and follow its instructions exactly. When you finish, make your final response your closing summary.\" \\".to_string(),
        "      </dev/null \\".to_string(),
        "      > \"$outputs_dir/claude-events.jsonl\" \\".to_string(),
        "      2> \"$outputs_dir/claude-stderr.log\"".to_string(),
        "  ' sh {}".to_string(),
    ]
    .join("\n")
}

/// Judge dispatch recipe over `judge-tasks.json`, one `claude -p` per task.
pub(crate) fn claude_judge_dispatch_recipe(model_flag: Option<&str>) -> String {
    let model_flag = model_flag.unwrap_or("--model");
    [
        "Dispatch each judge task from judge-tasks.json with:".to_string(),
        String::new(),
        "```bash".to_string(),
        "JOBS=${JOBS:-4}".to_string(),
        "jq -j '.tasks[] | [.dispatch_prompt_path, .response_path, (.model // \"\")] | @tsv + \"\\u0000\"' judge-tasks.json | \\".to_string(),
        "  xargs -0 -P \"$JOBS\" -I{} sh -c '".to_string(),
        "    prompt_path=\"$(printf \"%s\" \"$1\" | cut -f1)\"".to_string(),
        "    response_path=\"$(printf \"%s\" \"$1\" | cut -f2)\"".to_string(),
        "    model=\"$(printf \"%s\" \"$1\" | cut -f3)\"".to_string(),
        "    response_base=\"${response_path%.json}\"".to_string(),
        "    mkdir -p \"$(dirname \"$response_path\")\"".to_string(),
        "    model_arg=\"\"; [ -n \"$model\" ] && model_arg=\"".to_string()
            + model_flag
            + " $model\"",
        "    cd <eval-root> && claude -p --output-format stream-json --verbose --permission-mode acceptEdits $model_arg \\".to_string(),
        "      \"Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers.\" \\".to_string(),
        "      </dev/null \\".to_string(),
        "      > \"$response_base.claude-events.jsonl\" \\".to_string(),
        "      2> \"$response_base.claude-stderr.log\"".to_string(),
        "  ' sh {}".to_string(),
        "```".to_string(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        claude_exec_command_template, claude_judge_dispatch_recipe, claude_parallel_dispatch_recipe,
    };

    #[test]
    fn exec_template_carries_required_stream_json_flags() {
        let cmd = claude_exec_command_template(Some("--model"), None);
        assert!(cmd.contains("claude -p"), "{cmd}");
        assert!(cmd.contains("--output-format stream-json"), "{cmd}");
        // stream-json requires --verbose in -p mode.
        assert!(cmd.contains("--verbose"), "{cmd}");
        assert!(cmd.contains("--permission-mode acceptEdits"), "{cmd}");
        assert!(cmd.contains("> <outputs_dir>/claude-events.jsonl"), "{cmd}");
        assert!(cmd.contains("2> <outputs_dir>/claude-stderr.log"), "{cmd}");
        assert!(cmd.contains("</dev/null"), "{cmd}");
        // claude has no --cd flag; the dispatch runs from the env dir.
        assert!(cmd.contains("cd <eval-root>"), "{cmd}");
        assert!(cmd.contains("<dispatch_prompt_path>"), "{cmd}");
        // claude has no --output-last-message; final text comes from the result event.
        assert!(!cmd.contains("--output-last-message"), "{cmd}");
        assert!(!cmd.contains("final-message.md"), "{cmd}");
    }

    #[test]
    fn exec_template_includes_model_only_when_declared() {
        let with = claude_exec_command_template(Some("--model"), Some("opus"));
        assert!(with.contains("--model opus"), "{with}");
        let without = claude_exec_command_template(Some("--model"), None);
        assert!(!without.contains("--model "), "{without}");
    }

    #[test]
    fn parallel_recipe_drives_claude_p_per_task() {
        let recipe = claude_parallel_dispatch_recipe(Some("--model"), Some("sonnet"));
        assert!(recipe.contains("claude -p"), "{recipe}");
        assert!(recipe.contains("claude-events.jsonl"), "{recipe}");
        assert!(recipe.contains("dispatch.json"), "{recipe}");
        assert!(recipe.contains("--model sonnet"), "{recipe}");
    }

    #[test]
    fn judge_recipe_drives_claude_p() {
        let recipe = claude_judge_dispatch_recipe(Some("--model"));
        assert!(recipe.contains("claude -p"), "{recipe}");
        assert!(recipe.contains("judge-tasks.json"), "{recipe}");
        assert!(recipe.contains("response_path"), "{recipe}");
    }
}
