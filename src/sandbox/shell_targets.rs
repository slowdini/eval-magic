//! Quote-aware shell scanning for output redirection and `tee` targets.

use std::path::Path;

use super::policy::{BashClassification, OUTPUT_REDIRECTION_REASON, is_under_any, resolve_path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ShellWord {
    pub(super) value: String,
    pub(super) dynamic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ShellToken {
    Word(ShellWord),
    OutputRedirect,
    /// A file-descriptor duplication or close (`2>&1`, `>&2`, `>&-`). It routes
    /// one descriptor onto another and never opens a file, so it is not output
    /// redirection.
    FdDuplicate,
    InputRedirect,
    Pipe,
    Separator,
}

pub(super) struct LexedShell {
    pub(super) tokens: Vec<ShellToken>,
    pub(super) malformed: bool,
}

/// Split just enough shell syntax to locate literal output targets. Quotes and
/// backslash escapes are removed from words; expansion/glob syntax is marked
/// dynamic so it can be denied without executing a shell.
pub(super) fn lex_shell(command: &str) -> LexedShell {
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
                let mut j = i + 1;
                if j < chars.len() && matches!(chars[j], '>' | '|') {
                    j += 1;
                }
                match fd_duplication_end(&chars, j) {
                    Some(end) => {
                        tokens.push(ShellToken::FdDuplicate);
                        i = end;
                    }
                    None => {
                        tokens.push(ShellToken::OutputRedirect);
                        i = j;
                    }
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

/// End index of a file-descriptor duplication or close operand beginning at
/// `at` (`&1` in `2>&1`, `&-` in `>&-`), or `None` when the `>` opens a file.
///
/// Bash's `>&word` form with a non-numeric word redirects both streams to that
/// *file*, so anything other than digits or `-` deliberately falls through to
/// the file-output path and stays subject to the allowed-root check.
fn fd_duplication_end(chars: &[char], at: usize) -> Option<usize> {
    if chars.get(at) != Some(&'&') {
        return None;
    }
    let mut k = at + 1;
    if chars.get(k) == Some(&'-') {
        return Some(k + 1);
    }
    while chars.get(k).is_some_and(|c| c.is_ascii_digit()) {
        k += 1;
    }
    (k > at + 1).then_some(k)
}

/// True for a path that names a stream rather than a file on disk. Redirecting
/// to one writes nothing to the filesystem, so it is never a write target.
/// Applied to the *resolved* path, so `/dev/../etc/passwd` cannot launder an
/// out-of-bounds target through the `/dev` prefix.
fn is_non_file_device(resolved: &Path) -> bool {
    let Ok(rest) = resolved.strip_prefix("/dev") else {
        return false;
    };
    match rest.to_str() {
        Some("null" | "stdout" | "stderr") => true,
        Some(other) => other
            .strip_prefix("fd/")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
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
    if is_non_file_device(&resolved) {
        return true;
    }
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
                // A duplication carries its whole operand, so there is no
                // following target word to step over.
                ShellToken::FdDuplicate => {
                    j += 1;
                    continue;
                }
                ShellToken::Word(word) => {
                    if word.value.chars().all(|c| c.is_ascii_digit())
                        && matches!(
                            tokens.get(j + 1),
                            Some(ShellToken::OutputRedirect | ShellToken::FdDuplicate)
                        )
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

#[cfg(test)]
mod tests {
    use super::*;

    const ROOTS: [&str; 1] = ["/work/env"];

    fn classify(command: &str) -> Option<BashClassification> {
        classify_output_targets(command, &ROOTS.map(str::to_string), Path::new("/work/env"))
    }

    fn tokens(command: &str) -> Vec<ShellToken> {
        lex_shell(command).tokens
    }

    fn word(value: &str) -> ShellToken {
        ShellToken::Word(ShellWord {
            value: value.to_string(),
            dynamic: false,
        })
    }

    #[test]
    fn lexer_reads_fd_duplication_as_its_own_token() {
        assert_eq!(
            tokens("cmd 2>&1"),
            vec![word("cmd"), word("2"), ShellToken::FdDuplicate]
        );
        assert_eq!(
            tokens("cmd >&2"),
            vec![word("cmd"), ShellToken::FdDuplicate]
        );
        assert_eq!(
            tokens("cmd >&-"),
            vec![word("cmd"), ShellToken::FdDuplicate]
        );
    }

    #[test]
    fn lexer_keeps_treating_ampersand_before_a_redirect_as_file_output() {
        // `&>file` redirects both streams *to a file* — the leading `&` is a
        // separator, not part of the redirect operator.
        assert_eq!(
            tokens("cmd &>out.txt"),
            vec![
                word("cmd"),
                ShellToken::Separator,
                ShellToken::OutputRedirect,
                word("out.txt")
            ]
        );
    }

    #[test]
    fn lexer_keeps_appending_and_clobbering_redirects_distinct_from_duplication() {
        assert_eq!(
            tokens("cmd 2>>log.txt"),
            vec![
                word("cmd"),
                word("2"),
                ShellToken::OutputRedirect,
                word("log.txt")
            ]
        );
        assert_eq!(
            tokens("cmd >|out.txt"),
            vec![word("cmd"), ShellToken::OutputRedirect, word("out.txt")]
        );
    }

    #[test]
    fn fd_duplication_alone_requests_no_file_output() {
        assert_eq!(classify("git status 2>&1 | head -20"), None);
        assert_eq!(classify("printf done >&2"), None);
    }

    #[test]
    fn fd_duplication_does_not_poison_an_in_bounds_redirect() {
        assert_eq!(classify("printf done > out.txt 2>&1"), None);
    }

    #[test]
    fn non_file_devices_are_not_recorded_as_targets() {
        assert_eq!(classify("ls fixtures 2>/dev/null"), None);
        assert_eq!(classify("printf done >/dev/null 2>&1"), None);
        assert_eq!(classify("printf done | tee /dev/null"), None);
    }

    #[test]
    fn an_out_of_bounds_target_still_denies_alongside_a_duplication() {
        let denial = classify("printf done > /etc/out.txt 2>&1").expect("should deny");
        assert_eq!(denial.reason, OUTPUT_REDIRECTION_REASON);
        assert_eq!(denial.resolved_targets, vec!["/etc/out.txt".to_string()]);
    }

    #[test]
    fn a_dynamic_target_still_denies_with_no_resolved_target() {
        let denial = classify("printf done > \"$OUT\" 2>&1").expect("should deny");
        assert!(denial.resolved_targets.is_empty());
    }

    #[test]
    fn a_dev_prefix_does_not_launder_an_out_of_bounds_target() {
        for command in [
            "printf done > /dev/nullx",
            "printf done > /dev/sda",
            "printf done > /dev/../etc/passwd",
        ] {
            assert!(classify(command).is_some(), "should deny: {command}");
        }
    }
}
