//! Codex `exec` command guidance shared by stdout summaries and manifests.

/// Copy/pasteable Codex dispatch command template. Stdin is detached so a
/// surrounding `xargs`/pipe cannot be treated as extra prompt context.
pub(crate) fn codex_exec_command_template(guard: bool) -> String {
    let hook_trust = if guard {
        " --dangerously-bypass-hook-trust"
    } else {
        ""
    };
    [
        format!(
            "codex exec --cd <eval-root> --sandbox workspace-write --ask-for-approval never{hook_trust} --json \\"
        ),
        "  --output-last-message <outputs_dir>/final-message.md \\".to_string(),
        "  \"Read the file at <dispatch_prompt_path> and follow its instructions exactly. When you finish, make your final response exactly the same text you wrote to <outputs_dir>/final-message.md.\" \\".to_string(),
        "  </dev/null \\".to_string(),
        "  > <outputs_dir>/codex-events.jsonl \\".to_string(),
        "  2> <outputs_dir>/codex-stderr.log".to_string(),
    ]
    .join("\n")
}

pub(crate) fn codex_parallel_dispatch_recipe(guard: bool) -> String {
    let hook_trust = if guard {
        " --dangerously-bypass-hook-trust"
    } else {
        ""
    };
    [
        "JOBS=${JOBS:-4}".to_string(),
        "jq -j '.tasks[] | [.dispatch_prompt_path, .outputs_dir] | @tsv + \"\\u0000\"' dispatch.json | \\".to_string(),
        "  xargs -0 -P \"$JOBS\" -I{} sh -c '".to_string(),
        "    prompt_path=\"$(printf \"%s\" \"$1\" | cut -f1)\"".to_string(),
        "    outputs_dir=\"$(printf \"%s\" \"$1\" | cut -f2)\"".to_string(),
        "    mkdir -p \"$outputs_dir\"".to_string(),
        format!(
            "    codex exec --cd <eval-root> --sandbox workspace-write --ask-for-approval never{hook_trust} --json \\"
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
