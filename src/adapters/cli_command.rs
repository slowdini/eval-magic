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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{render_agent_dispatch_command, render_cli_model_arg, shell_quote_arg};

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
}
