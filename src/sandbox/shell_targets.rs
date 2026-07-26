//! Quote-aware shell scanning for output redirection and `tee` targets.

use std::path::Path;

use super::policy::{BashClassification, OUTPUT_REDIRECTION_REASON, is_under_any, resolve_path};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellWord {
    value: String,
    dynamic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ShellToken {
    Word(ShellWord),
    OutputRedirect,
    InputRedirect,
    Pipe,
    Separator,
}

struct LexedShell {
    tokens: Vec<ShellToken>,
    malformed: bool,
}

/// Split just enough shell syntax to locate literal output targets. Quotes and
/// backslash escapes are removed from words; expansion/glob syntax is marked
/// dynamic so it can be denied without executing a shell.
fn lex_shell(command: &str) -> LexedShell {
    let chars: Vec<char> = command.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut malformed = false;

    while i < chars.len() {
        match chars[i] {
            c if c.is_whitespace() => {
                if c == '\n' {
                    tokens.push(ShellToken::Separator);
                }
                i += 1;
            }
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '>' => {
                tokens.push(ShellToken::OutputRedirect);
                i += 1;
                if i < chars.len() && matches!(chars[i], '>' | '|') {
                    i += 1;
                }
            }
            '<' => {
                tokens.push(ShellToken::InputRedirect);
                i += 1;
                while i < chars.len() && chars[i] == '<' {
                    i += 1;
                }
            }
            '|' => {
                i += 1;
                if i < chars.len() && chars[i] == '|' {
                    i += 1;
                    tokens.push(ShellToken::Separator);
                } else {
                    tokens.push(ShellToken::Pipe);
                }
            }
            ';' => {
                tokens.push(ShellToken::Separator);
                i += 1;
            }
            '&' => {
                tokens.push(ShellToken::Separator);
                i += 1;
                if i < chars.len() && chars[i] == '&' {
                    i += 1;
                }
            }
            _ => {
                let mut value = String::new();
                let mut dynamic = false;
                let mut started = false;

                while i < chars.len() {
                    let c = chars[i];
                    if c.is_whitespace() || matches!(c, '>' | '<' | '|' | ';' | '&') {
                        break;
                    }
                    started = true;
                    match c {
                        '\'' => {
                            i += 1;
                            let mut closed = false;
                            while i < chars.len() {
                                if chars[i] == '\'' {
                                    i += 1;
                                    closed = true;
                                    break;
                                }
                                value.push(chars[i]);
                                i += 1;
                            }
                            if !closed {
                                malformed = true;
                                break;
                            }
                        }
                        '"' => {
                            i += 1;
                            let mut closed = false;
                            while i < chars.len() {
                                match chars[i] {
                                    '"' => {
                                        i += 1;
                                        closed = true;
                                        break;
                                    }
                                    '\\' => {
                                        i += 1;
                                        if i == chars.len() {
                                            malformed = true;
                                            break;
                                        }
                                        value.push(chars[i]);
                                        i += 1;
                                    }
                                    '$' | '`' => {
                                        dynamic = true;
                                        value.push(chars[i]);
                                        i += 1;
                                    }
                                    other => {
                                        value.push(other);
                                        i += 1;
                                    }
                                }
                            }
                            if !closed {
                                malformed = true;
                                break;
                            }
                        }
                        '\\' => {
                            i += 1;
                            if i == chars.len() {
                                malformed = true;
                                break;
                            }
                            value.push(chars[i]);
                            i += 1;
                        }
                        '$' | '`' | '*' | '?' | '[' | '{' | '}' | '(' | ')' => {
                            dynamic = true;
                            value.push(c);
                            i += 1;
                        }
                        '~' if value.is_empty() => {
                            dynamic = true;
                            value.push(c);
                            i += 1;
                        }
                        other => {
                            value.push(other);
                            i += 1;
                        }
                    }
                    if malformed {
                        break;
                    }
                }

                if started {
                    tokens.push(ShellToken::Word(ShellWord { value, dynamic }));
                }
                if malformed {
                    break;
                }
            }
        }
    }

    LexedShell { tokens, malformed }
}

fn is_tee_command(word: &ShellWord) -> bool {
    Path::new(&word.value)
        .file_name()
        .is_some_and(|name| name == "tee")
        && !word.dynamic
}

fn record_literal_target(
    word: &ShellWord,
    allowed_roots: &[String],
    invocation_cwd: &Path,
    resolved_targets: &mut Vec<String>,
) -> bool {
    if word.dynamic || word.value.is_empty() || word.value.contains('\0') {
        return false;
    }
    let resolved = resolve_path(&word.value, invocation_cwd);
    resolved_targets.push(resolved.display().to_string());
    is_under_any(&word.value, allowed_roots, invocation_cwd)
}

/// Return a denial when an output redirect or `tee` target cannot be proven to
/// be a literal path inside an allowed root. `None` means either no file output
/// was requested or every discovered target is in bounds.
pub(super) fn classify_output_targets(
    command: &str,
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    let lexed = lex_shell(command);
    let tokens = &lexed.tokens;
    let mut saw_file_output = false;
    let mut unsafe_target = false;
    let mut resolved_targets = Vec::new();

    for (i, token) in tokens.iter().enumerate() {
        if !matches!(token, ShellToken::OutputRedirect) {
            continue;
        }
        saw_file_output = true;
        match tokens.get(i + 1) {
            Some(ShellToken::Word(word)) => {
                if !record_literal_target(
                    word,
                    allowed_roots,
                    invocation_cwd,
                    &mut resolved_targets,
                ) {
                    unsafe_target = true;
                }
            }
            _ => unsafe_target = true,
        }
    }

    for i in 0..tokens.len() {
        let ShellToken::Word(command_word) = &tokens[i] else {
            continue;
        };
        // Wrappers such as `sudo tee` and `env tee` are common. Treat any
        // literal command word named `tee` as the start of its target list,
        // matching the old conservative heuristic while validating paths.
        if !is_tee_command(command_word) {
            continue;
        }

        let mut options = true;
        let mut j = i + 1;
        while j < tokens.len() {
            match &tokens[j] {
                ShellToken::Pipe | ShellToken::Separator => break,
                ShellToken::OutputRedirect | ShellToken::InputRedirect => {
                    j += 2;
                    continue;
                }
                ShellToken::Word(word) => {
                    if word.value.chars().all(|c| c.is_ascii_digit())
                        && matches!(tokens.get(j + 1), Some(ShellToken::OutputRedirect))
                    {
                        j += 1;
                        continue;
                    }
                    if word.dynamic {
                        saw_file_output = true;
                        unsafe_target = true;
                        j += 1;
                        continue;
                    }
                    if options && word.value == "--" {
                        options = false;
                        j += 1;
                        continue;
                    }
                    if options && word.value.starts_with('-') && word.value != "-" {
                        j += 1;
                        continue;
                    }
                    options = false;
                    if word.value != "-" {
                        saw_file_output = true;
                        if !record_literal_target(
                            word,
                            allowed_roots,
                            invocation_cwd,
                            &mut resolved_targets,
                        ) {
                            unsafe_target = true;
                        }
                    }
                    j += 1;
                }
            }
        }
    }

    resolved_targets.dedup();
    if saw_file_output && (unsafe_target || lexed.malformed) {
        Some(BashClassification {
            reason: OUTPUT_REDIRECTION_REASON,
            resolved_targets,
        })
    } else {
        None
    }
}
