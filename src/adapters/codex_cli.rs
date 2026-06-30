//! Codex CLI command rendering (`codex exec`) for dispatch guidance.

use super::cli_command::render_cli_model_arg;
use std::path::Path;

/// Copy/pasteable Codex dispatch command template. Stdin is detached so a
/// surrounding `xargs`/pipe cannot be treated as extra prompt context.
pub(crate) fn codex_exec_command_template(
    model_flag: Option<&str>,
    guard: bool,
    agent_model: Option<&str>,
) -> String {
    let hook_trust = if guard {
        " --dangerously-bypass-hook-trust"
    } else {
        ""
    };
    let model_arg = render_cli_model_arg(model_flag, agent_model);
    [
        format!(
            "codex --ask-for-approval never exec --cd <eval-root> --sandbox workspace-write{hook_trust}{model_arg} --json \\"
        ),
        "  --output-last-message <outputs_dir>/final-message.md \\".to_string(),
        "  \"Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to <outputs_dir>/final-message.md.\" \\".to_string(),
        "  </dev/null \\".to_string(),
        "  > <outputs_dir>/codex-events.jsonl \\".to_string(),
        "  2> <outputs_dir>/codex-stderr.log".to_string(),
    ]
    .join("\n")
}

pub(crate) fn codex_parallel_dispatch_recipe(
    model_flag: Option<&str>,
    guard: bool,
    agent_model: Option<&str>,
) -> String {
    let hook_trust = if guard {
        " --dangerously-bypass-hook-trust"
    } else {
        ""
    };
    let model_arg = render_cli_model_arg(model_flag, agent_model);
    [
        "JOBS=${JOBS:-4}".to_string(),
        "jq -j '.tasks[] | [.eval_root, .dispatch_prompt_path, .outputs_dir] | @tsv + \"\\u0000\"' dispatch.json | \\".to_string(),
        "  xargs -0 -P \"$JOBS\" -I{} sh -c '".to_string(),
        "    eval_root=\"$(printf \"%s\" \"$1\" | cut -f1)\"".to_string(),
        "    prompt_path=\"$(printf \"%s\" \"$1\" | cut -f2)\"".to_string(),
        "    outputs_dir=\"$(printf \"%s\" \"$1\" | cut -f3)\"".to_string(),
        "    mkdir -p \"$outputs_dir\"".to_string(),
        format!(
            "    codex --ask-for-approval never exec --cd \"$eval_root\" --sandbox workspace-write{hook_trust}{model_arg} --json \\"
        ),
        "      --output-last-message \"$outputs_dir/final-message.md\" \\".to_string(),
        "      \"Read the file at $prompt_path and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to $outputs_dir/final-message.md.\" \\".to_string(),
        "      </dev/null \\".to_string(),
        "      > \"$outputs_dir/codex-events.jsonl\" \\".to_string(),
        "      2> \"$outputs_dir/codex-stderr.log\"".to_string(),
        "  ' sh {}".to_string(),
    ]
    .join("\n")
}

/// Judges run from `judge_cwd` (the iteration dir) — a common ancestor of every
/// judge prompt, verdict `response_path`, and agent `outputs_dir`.
pub(crate) fn codex_judge_dispatch_recipe(
    model_flag: Option<&str>,
    guard: bool,
    judge_cwd: &Path,
) -> String {
    let hook_trust = if guard {
        " --dangerously-bypass-hook-trust"
    } else {
        ""
    };
    let model_flag = model_flag.unwrap_or("-m");
    let cwd = judge_cwd.display();
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
        "    if [ -n \"$model\" ]; then".to_string(),
        format!(
            "      codex --ask-for-approval never exec --cd \"{cwd}\" --sandbox workspace-write{hook_trust} {model_flag} \"$model\" --json \\"
        ),
        "        \"Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers.\" \\".to_string(),
        "        </dev/null \\".to_string(),
        "        > \"$response_base.codex-events.jsonl\" \\".to_string(),
        "        2> \"$response_base.codex-stderr.log\"".to_string(),
        "    else".to_string(),
        format!(
            "      codex --ask-for-approval never exec --cd \"{cwd}\" --sandbox workspace-write{hook_trust} --json \\"
        ),
        "        \"Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers.\" \\".to_string(),
        "        </dev/null \\".to_string(),
        "        > \"$response_base.codex-events.jsonl\" \\".to_string(),
        "        2> \"$response_base.codex-stderr.log\"".to_string(),
        "    fi".to_string(),
        "  ' sh {}".to_string(),
        "```".to_string(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        codex_exec_command_template, codex_judge_dispatch_recipe, codex_parallel_dispatch_recipe,
    };
    use std::path::Path;

    #[test]
    fn exec_template_places_approval_policy_before_exec() {
        let cmd = codex_exec_command_template(Some("-m"), true, Some("gpt-5-mini"));
        let first_line = cmd.lines().next().unwrap();

        assert_eq!(
            first_line,
            "codex --ask-for-approval never exec --cd <eval-root> --sandbox workspace-write --dangerously-bypass-hook-trust -m gpt-5-mini --json \\"
        );
    }

    #[test]
    fn parallel_recipe_places_approval_policy_before_exec() {
        let recipe = codex_parallel_dispatch_recipe(Some("-m"), true, Some("gpt-5-mini"));

        assert!(
            recipe.contains(
                "    codex --ask-for-approval never exec --cd \"$eval_root\" --sandbox workspace-write --dangerously-bypass-hook-trust -m gpt-5-mini --json \\"
            ),
            "{recipe}"
        );
    }

    #[test]
    fn judge_recipe_places_approval_policy_before_exec() {
        let recipe = codex_judge_dispatch_recipe(Some("-m"), true, Path::new("/work/iter-1"));

        assert!(
            recipe.contains(
                "      codex --ask-for-approval never exec --cd \"/work/iter-1\" --sandbox workspace-write --dangerously-bypass-hook-trust -m \"$model\" --json \\"
            ),
            "{recipe}"
        );
        assert!(
            recipe.contains(
                "      codex --ask-for-approval never exec --cd \"/work/iter-1\" --sandbox workspace-write --dangerously-bypass-hook-trust --json \\"
            ),
            "{recipe}"
        );
        assert!(!recipe.contains("<eval-root>"), "{recipe}");
    }
}
