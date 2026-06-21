//! Codex CLI command rendering for `DispatchMechanism::Cli` guidance.

use super::cli_command::render_cli_model_arg;

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
            "codex exec --cd <eval-root> --sandbox workspace-write --ask-for-approval never{hook_trust}{model_arg} --json \\"
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
            "    codex exec --cd \"$eval_root\" --sandbox workspace-write --ask-for-approval never{hook_trust}{model_arg} --json \\"
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

pub(crate) fn codex_judge_dispatch_recipe(model_flag: Option<&str>, guard: bool) -> String {
    let hook_trust = if guard {
        " --dangerously-bypass-hook-trust"
    } else {
        ""
    };
    let model_flag = model_flag.unwrap_or("-m");
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
            "      codex exec --cd <eval-root> --sandbox workspace-write --ask-for-approval never{hook_trust} {model_flag} \"$model\" --json \\"
        ),
        "        \"Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers.\" \\".to_string(),
        "        </dev/null \\".to_string(),
        "        > \"$response_base.codex-events.jsonl\" \\".to_string(),
        "        2> \"$response_base.codex-stderr.log\"".to_string(),
        "    else".to_string(),
        format!(
            "      codex exec --cd <eval-root> --sandbox workspace-write --ask-for-approval never{hook_trust} --json \\"
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
