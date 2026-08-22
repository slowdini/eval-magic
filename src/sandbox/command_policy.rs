//! Configured shell-command allowances for the write guard.

use std::path::Path;

use crate::core::GuardPolicyConfig;

use super::policy::BashClassification;
use super::shell_targets::{ShellToken, ShellWord, lex_shell};

pub(super) const COMMAND_POLICY_REASON: &str = "command not allowed by eval guard policy";

pub(crate) fn validate_policy_syntax(policy: &GuardPolicyConfig) -> Result<(), String> {
    for tool in &policy.allow_tools {
        if tool.is_empty()
            || tool
                != Path::new(tool)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("")
            || !tool
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-+_.".contains(&byte))
        {
            return Err(format!(
                "allow_tools entry {tool:?} must be a literal executable basename"
            ));
        }
    }
    for rule in &policy.allow_commands {
        let lexed = lex_shell(rule);
        if lexed.malformed
            || lexed.tokens.is_empty()
            || lexed
                .tokens
                .iter()
                .any(|token| !matches!(token, ShellToken::Word(word) if !word.dynamic))
        {
            return Err(format!(
                "allow_commands entry {rule:?} must be one literal shell command without operators, redirects, or expansions"
            ));
        }
        let words: Vec<ShellWord> = lexed
            .tokens
            .into_iter()
            .filter_map(|token| match token {
                ShellToken::Word(word) => Some(word),
                _ => None,
            })
            .collect();
        if words.first().is_some_and(is_assignment) || normalized_words(&words).is_none() {
            return Err(format!(
                "allow_commands entry {rule:?} must begin with a literal executable"
            ));
        }
    }
    Ok(())
}

fn executable_name(word: &ShellWord) -> Option<&str> {
    (!word.dynamic)
        .then(|| Path::new(&word.value).file_name()?.to_str())
        .flatten()
}

fn is_assignment(word: &ShellWord) -> bool {
    !word.dynamic
        && word
            .value
            .split_once('=')
            .is_some_and(|(name, _)| !name.is_empty() && !name.contains('/'))
}

fn command_words(command: &str) -> Option<Vec<ShellWord>> {
    let lexed = lex_shell(command);
    if lexed.malformed {
        return None;
    }
    let mut words = Vec::new();
    for token in lexed.tokens {
        match token {
            ShellToken::Word(word) => words.push(word),
            ShellToken::InputRedirect | ShellToken::FdDuplicate => {}
            ShellToken::OutputRedirect | ShellToken::Pipe | ShellToken::Separator => return None,
        }
    }
    Some(words)
}

enum NormalizedCommand {
    Words(Vec<String>),
    Script(String),
}

fn normalized_command(words: &[ShellWord]) -> Option<NormalizedCommand> {
    let command_index = words.iter().position(|word| !is_assignment(word))?;
    let mut literal = Vec::with_capacity(words.len() - command_index);
    literal.push(executable_name(&words[command_index])?.to_string());
    for word in &words[command_index + 1..] {
        if word.dynamic {
            literal.push("\0dynamic".to_string());
            continue;
        }
        literal.push(word.value.clone());
    }
    let mut command = 0;

    loop {
        match literal.get(command)?.as_str() {
            "env" => {
                command += 1;
                while let Some(word) = literal.get(command) {
                    if word == "--" {
                        command += 1;
                        break;
                    }
                    if word == "-u" || word == "--unset" || word == "-C" || word == "--chdir" {
                        command += 2;
                    } else if word.starts_with('-') || assignment_value(word) {
                        command += 1;
                    } else {
                        break;
                    }
                }
            }
            "command" => {
                command += 1;
                while literal
                    .get(command)
                    .is_some_and(|word| word.starts_with('-'))
                {
                    command += 1;
                }
            }
            "exec" => {
                command += 1;
                while let Some(word) = literal.get(command) {
                    if word == "-a" {
                        command += 2;
                    } else if word.starts_with('-') {
                        command += 1;
                    } else {
                        break;
                    }
                }
            }
            "nice" => {
                command += 1;
                while let Some(word) = literal.get(command) {
                    if word == "-n" || word == "--adjustment" {
                        command += 2;
                    } else if word.starts_with('-') {
                        command += 1;
                    } else {
                        break;
                    }
                }
            }
            "timeout" => {
                command += 1;
                while let Some(word) = literal.get(command) {
                    if matches!(word.as_str(), "-s" | "--signal" | "-k" | "--kill-after") {
                        command += 2;
                    } else if word.starts_with('-') {
                        command += 1;
                    } else {
                        break;
                    }
                }
                command += 1; // duration
            }
            "sh" | "bash" | "zsh" if literal.get(command + 1).is_some_and(|word| word == "-c") => {
                return Some(NormalizedCommand::Script(literal.get(command + 2)?.clone()));
            }
            _ => break,
        }
    }

    let executable = Path::new(literal.get(command)?)
        .file_name()?
        .to_str()?
        .to_string();
    let mut out = Vec::with_capacity(literal.len() - command);
    out.push(executable);
    out.extend(literal[command + 1..].iter().cloned());
    Some(NormalizedCommand::Words(out))
}

fn normalized_words(words: &[ShellWord]) -> Option<Vec<String>> {
    match normalized_command(words)? {
        NormalizedCommand::Words(words) => Some(words),
        NormalizedCommand::Script(script) => parsed_rule(&script),
    }
}

fn assignment_value(word: &str) -> bool {
    word.split_once('=')
        .is_some_and(|(name, _)| !name.is_empty() && !name.contains('/'))
}

fn parsed_rule(rule: &str) -> Option<Vec<String>> {
    normalized_words(&command_words(rule)?)
}

fn segment_denied(words: &[ShellWord], policy: &GuardPolicyConfig, malformed: bool) -> bool {
    if words
        .iter()
        .find(|word| !is_assignment(word))
        .and_then(executable_name)
        .is_some_and(|tool| policy.allow_tools.iter().any(|allowed| allowed == tool))
    {
        return false;
    }
    let Some(normalized) = normalized_command(words) else {
        return false;
    };
    let actual = match normalized {
        NormalizedCommand::Words(words) => words,
        NormalizedCommand::Script(script) => {
            return malformed || classify_command_policy(&script, policy).is_some();
        }
    };
    let Some(tool) = actual.first() else {
        return false;
    };

    if policy.allow_tools.iter().any(|allowed| allowed == tool) {
        return false;
    }

    let mut claimed = false;
    for rule in &policy.allow_commands {
        let Some(rule) = parsed_rule(rule) else {
            continue;
        };
        if rule.first() == Some(tool) {
            claimed = true;
            if !malformed && actual.starts_with(&rule) {
                return false;
            }
        }
    }
    claimed
        || super::guard_profiles::claims_command(&actual)
        || super::mutation_targets::segment_is_recognized(words)
}

pub(super) fn literal_shell_scripts(command: &str) -> Vec<String> {
    let lexed = lex_shell(command);
    let mut scripts = Vec::new();
    let mut segment = Vec::new();
    let collect = |segment: &[ShellWord], scripts: &mut Vec<String>| {
        if let Some(NormalizedCommand::Script(script)) = normalized_command(segment)
            && !script.contains('\0')
        {
            scripts.push(script);
        }
    };
    for token in lexed.tokens {
        match token {
            ShellToken::Word(word) => segment.push(word),
            ShellToken::Pipe | ShellToken::Separator => {
                collect(&segment, &mut scripts);
                segment.clear();
            }
            ShellToken::OutputRedirect | ShellToken::FdDuplicate | ShellToken::InputRedirect => {}
        }
    }
    collect(&segment, &mut scripts);
    scripts
}

pub(super) fn classify_command_policy(
    command: &str,
    policy: &GuardPolicyConfig,
) -> Option<BashClassification> {
    let lexed = lex_shell(command);
    let mut segment = Vec::new();
    for token in lexed.tokens {
        match token {
            ShellToken::Word(word) => segment.push(word),
            ShellToken::Pipe | ShellToken::Separator => {
                if segment_denied(&segment, policy, lexed.malformed) {
                    return Some(BashClassification {
                        reason: COMMAND_POLICY_REASON,
                        resolved_targets: Vec::new(),
                    });
                }
                segment.clear();
            }
            ShellToken::OutputRedirect | ShellToken::FdDuplicate | ShellToken::InputRedirect => {}
        }
    }
    segment_denied(&segment, policy, lexed.malformed).then(|| BashClassification {
        reason: COMMAND_POLICY_REASON,
        resolved_targets: Vec::new(),
    })
}
