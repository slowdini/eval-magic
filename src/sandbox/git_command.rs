//! Guard classification for Git commands embedded in shell tool calls.

use std::path::Path;

use crate::core::GIT_ROUTING_ENV_VARS;
use crate::core::fs::artifact_path;

use super::policy::{BashClassification, is_under_any, resolve_path};
use super::shell_targets::{ShellToken, ShellWord, lex_shell};

fn is_git_command(word: &ShellWord) -> bool {
    Path::new(&word.value)
        .file_name()
        .is_some_and(|name| name == "git")
        && !word.dynamic
}

fn git_denial(reason: &'static str, resolved_targets: Vec<String>) -> Option<BashClassification> {
    Some(BashClassification {
        reason,
        resolved_targets,
    })
}

fn routing_assignment(word: &ShellWord) -> Option<(&str, &str)> {
    let (name, value) = word.value.split_once('=')?;
    GIT_ROUTING_ENV_VARS
        .contains(&name)
        .then_some((name, value))
}

fn routing_value_is_allowed(
    name: &str,
    value: &str,
    dynamic: bool,
    cwd: &Path,
    allowed_roots: &[String],
    resolved_targets: &mut Vec<String>,
) -> bool {
    if dynamic || value.is_empty() || value.contains('\0') {
        return false;
    }
    let values: Vec<String> = if matches!(
        name,
        "GIT_ALTERNATE_OBJECT_DIRECTORIES" | "GIT_CEILING_DIRECTORIES"
    ) {
        std::env::split_paths(value)
            .filter(|part| !part.as_os_str().is_empty())
            .map(|part| part.to_string_lossy().into_owned())
            .collect()
    } else {
        vec![value.to_string()]
    };
    if values.is_empty() {
        return false;
    }
    values.into_iter().all(|value| {
        resolved_targets.push(artifact_path(&resolve_path(&value, cwd)));
        is_under_any(&value, allowed_roots, cwd)
    })
}

fn selector_value<'a>(
    word: &'a ShellWord,
    long_name: &str,
    short_name: Option<&str>,
) -> Option<Option<&'a str>> {
    if word.value == long_name || short_name.is_some_and(|short| word.value == short) {
        return Some(None);
    }
    if let Some(value) = word.value.strip_prefix(&format!("{long_name}=")) {
        return Some(Some(value));
    }
    if let Some(short) = short_name
        && let Some(value) = word.value.strip_prefix(short)
        && !value.is_empty()
    {
        return Some(Some(value));
    }
    None
}

fn config_mutates_remote(args: &[&ShellWord]) -> bool {
    let sensitive = |value: &str| {
        let value = value.to_ascii_lowercase();
        value.starts_with("remote.") || value.starts_with("url.")
    };
    if !args.iter().any(|word| sensitive(&word.value)) {
        return false;
    }
    if args.iter().any(|word| {
        matches!(
            word.value.as_str(),
            "--add"
                | "--replace-all"
                | "--unset"
                | "--unset-all"
                | "--rename-section"
                | "--remove-section"
                | "--edit"
                | "-e"
        )
    }) {
        return true;
    }

    let positional: Vec<&str> = args
        .iter()
        .filter(|word| !word.value.starts_with('-'))
        .map(|word| word.value.as_str())
        .collect();
    if positional.first().is_some_and(|action| {
        matches!(
            *action,
            "set" | "unset" | "rename-section" | "remove-section"
        )
    }) {
        return positional.iter().skip(1).any(|value| sensitive(value));
    }
    positional
        .first()
        .is_some_and(|key| sensitive(key) && positional.len() > 1)
}

fn git_operation_denial_reason(
    words: &[&ShellWord],
    subcommand_index: usize,
) -> Option<&'static str> {
    let subcommand = words[subcommand_index].value.as_str();
    let args: Vec<&ShellWord> = words.iter().skip(subcommand_index + 1).copied().collect();
    match subcommand {
        "clone" | "fetch" | "pull" | "push" | "ls-remote" | "submodule" | "send-email" | "svn"
        | "p4" => Some("git remote operation"),
        "archive" => args
            .iter()
            .any(|word| word.value == "--remote" || word.value.starts_with("--remote="))
            .then_some("git remote operation"),
        "worktree" => args
            .iter()
            .filter(|word| !word.value.starts_with('-'))
            .any(|word| word.value == "add")
            .then_some("git worktree add (working tree outside the sandbox)"),
        "remote" => {
            let action = args
                .iter()
                .find(|word| !matches!(word.value.as_str(), "-v" | "--verbose"));
            (!matches!(
                action.map(|word| word.value.as_str()),
                None | Some("get-url")
            ))
            .then_some("git remote operation")
        }
        "config" => config_mutates_remote(&args).then_some("git remote operation"),
        _ => None,
    }
}

fn classify_git_segment(
    words: &[&ShellWord],
    malformed: bool,
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    let git_index = words.iter().position(|word| is_git_command(word))?;
    let mut resolved_targets = Vec::new();

    for word in &words[..git_index] {
        if let Some((name, value)) = routing_assignment(word)
            && !routing_value_is_allowed(
                name,
                value,
                word.dynamic,
                invocation_cwd,
                allowed_roots,
                &mut resolved_targets,
            )
        {
            return git_denial("git repository routing escape", resolved_targets);
        }
    }

    let mut effective_cwd = invocation_cwd.to_path_buf();
    let mut index = git_index + 1;
    let mut subcommand_index = None;
    while index < words.len() {
        let word = words[index];
        if word.dynamic {
            return git_denial("git repository routing escape", resolved_targets);
        }
        let selector = selector_value(word, "--git-dir", None)
            .map(|value| ("GIT_DIR", value))
            .or_else(|| {
                selector_value(word, "--work-tree", None).map(|value| ("GIT_WORK_TREE", value))
            })
            .or_else(|| selector_value(word, "-C", Some("-C")).map(|value| ("-C", value)));
        if let Some((name, inline_value)) = selector {
            let (value, dynamic) = match inline_value {
                Some(value) => (value, word.dynamic),
                None => {
                    let Some(next) = words.get(index + 1) else {
                        return git_denial("git repository routing escape", resolved_targets);
                    };
                    index += 1;
                    (next.value.as_str(), next.dynamic)
                }
            };
            if !routing_value_is_allowed(
                name,
                value,
                dynamic,
                &effective_cwd,
                allowed_roots,
                &mut resolved_targets,
            ) {
                return git_denial("git repository routing escape", resolved_targets);
            }
            if name == "-C" {
                effective_cwd = resolve_path(value, &effective_cwd);
            }
            index += 1;
            continue;
        }
        if matches!(word.value.as_str(), "-c" | "--config-env") {
            let Some(_next) = words.get(index + 1) else {
                return git_denial("git repository routing escape", resolved_targets);
            };
            index += 2;
            continue;
        }
        if word.value.starts_with('-') {
            index += 1;
            continue;
        }
        subcommand_index = Some(index);
        break;
    }

    if malformed {
        return git_denial("git repository routing escape", resolved_targets);
    }

    if let Some(reason) =
        subcommand_index.and_then(|index| git_operation_denial_reason(words, index))
    {
        return git_denial(reason, resolved_targets);
    }
    None
}

/// Validate every literal Git invocation before the guard's broad
/// allowed-root shortcut. Local repository operations are allowed; repository
/// routing escapes and remote-capable operations are denied.
pub(super) fn classify_git_commands(
    command: &str,
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    let lexed = lex_shell(command);
    let mut segment = Vec::new();
    for token in &lexed.tokens {
        match token {
            ShellToken::Word(word) => segment.push(word),
            ShellToken::Pipe | ShellToken::Separator => {
                if let Some(denial) =
                    classify_git_segment(&segment, lexed.malformed, allowed_roots, invocation_cwd)
                {
                    return Some(denial);
                }
                segment.clear();
            }
            ShellToken::OutputRedirect | ShellToken::FdDuplicate | ShellToken::InputRedirect => {}
        }
    }
    classify_git_segment(&segment, lexed.malformed, allowed_roots, invocation_cwd)
}
