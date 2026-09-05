//! Quote-aware shell scanning for output redirection and `tee` targets.

use std::path::Path;

use crate::core::fs::artifact_path;

use super::policy::{BashClassification, OUTPUT_REDIRECTION_REASON, is_under_any, resolve_path};

mod skill_access;

pub(crate) use skill_access::command_reads_literal_path;

/// Every literal word of a shell command — the spellings a path comparison can
/// resolve.
///
/// Dynamic words (expansions, globs) are dropped because they name no single
/// path, and a malformed command yields none rather than words lexed from a
/// misread of it. Callers that must still catch those shapes keep their own
/// scan of the raw command text.
pub(crate) fn literal_words(command: &str) -> Vec<String> {
    let lexed = lex_shell(command);
    if lexed.malformed {
        return Vec::new();
    }
    lexed
        .tokens
        .into_iter()
        .filter_map(|token| match token {
            ShellToken::Word(word) if !word.dynamic => Some(word.value),
            _ => None,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ShellWord {
    pub(super) value: String,
    /// True whenever the word still has something to expand. Kept beside
    /// `dynamic_prefix` because a word a caller derived from a dynamic one is
    /// dynamic with no usable prefix.
    pub(super) dynamic: bool,
    /// What first made this word dynamic, and the literal text that preceded
    /// it. `None` for a literal word, and also for a word a caller derived from
    /// a dynamic one, where the recorded text would no longer line up.
    pub(super) dynamic_prefix: Option<DynamicPrefix>,
}

/// Where a word stopped being a literal path, and what kind of expansion took
/// over there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DynamicPrefix {
    /// The word's value as it stood before the first dynamic character.
    pub(super) literal: String,
    pub(super) kind: DynamicKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DynamicKind {
    /// `$`, a backtick, `~`, or a subshell. The result is unbounded: nothing in
    /// the word says where the expansion can land.
    Expansion,
    /// `*`, `?`, a character class, or a brace. The result is drawn from a
    /// directory listing, so the literal prefix bounds it.
    Glob,
}

impl ShellWord {
    /// A word with nothing left to expand — a spelling a classifier resolved
    /// for itself, such as the value inside `--opt=value`.
    pub(super) fn literal(value: &str) -> Self {
        Self {
            value: value.to_string(),
            dynamic: false,
            dynamic_prefix: None,
        }
    }

    /// A slice of `source` re-read as a word of its own. It inherits `source`'s
    /// dynamism but carries no prefix: the slice boundary need not line up with
    /// where the expansion began, so it stays unresolvable.
    pub(super) fn slice_of(value: &str, source: &ShellWord) -> Self {
        Self {
            value: value.to_string(),
            dynamic: source.dynamic,
            dynamic_prefix: None,
        }
    }
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

struct Heredoc {
    delimiter: String,
    strip_tabs: bool,
}

fn skip_heredoc_bodies(chars: &[char], mut i: usize, heredocs: &[Heredoc]) -> Option<usize> {
    for heredoc in heredocs {
        loop {
            let line_end = chars[i..]
                .iter()
                .position(|c| *c == '\n')
                .map_or(chars.len(), |offset| i + offset);
            let line = &chars[i..line_end];
            let line = if heredoc.strip_tabs {
                &line[line.iter().take_while(|c| **c == '\t').count()..]
            } else {
                line
            };

            if line.iter().copied().eq(heredoc.delimiter.chars()) {
                i = line_end + usize::from(line_end < chars.len());
                break;
            }
            if line_end == chars.len() {
                return None;
            }
            i = line_end + 1;
        }
    }
    Some(i)
}

/// Split just enough shell syntax to locate literal output targets. Quotes and
/// backslash escapes are removed from words; expansion/glob syntax is marked
/// dynamic so it can be denied without executing a shell. Heredoc bodies are
/// skipped as data after their declaration line, then lexing resumes after each
/// terminator.
pub(super) fn lex_shell(command: &str) -> LexedShell {
    let chars: Vec<char> = command.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut malformed = false;
    let mut awaiting_heredoc_delimiter = None;
    let mut heredocs = Vec::new();

    while i < chars.len() {
        match chars[i] {
            c if c.is_whitespace() => {
                if c == '\n' {
                    tokens.push(ShellToken::Separator);
                    if awaiting_heredoc_delimiter.take().is_some() {
                        malformed = true;
                    }
                    if !heredocs.is_empty() {
                        i += 1;
                        match skip_heredoc_bodies(&chars, i, &heredocs) {
                            Some(after_bodies) => i = after_bodies,
                            None => {
                                malformed = true;
                                i = chars.len();
                            }
                        }
                        heredocs.clear();
                        continue;
                    }
                }
                i += 1;
            }
            '#' => {
                if awaiting_heredoc_delimiter.take().is_some() {
                    malformed = true;
                }
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '>' => {
                if awaiting_heredoc_delimiter.take().is_some() {
                    malformed = true;
                }
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
                if awaiting_heredoc_delimiter.take().is_some() {
                    malformed = true;
                }
                tokens.push(ShellToken::InputRedirect);
                let start = i;
                while i < chars.len() && chars[i] == '<' {
                    i += 1;
                }
                if i - start == 2 {
                    let strip_tabs = chars.get(i) == Some(&'-');
                    i += usize::from(strip_tabs);
                    awaiting_heredoc_delimiter = Some(strip_tabs);
                }
            }
            '|' => {
                if awaiting_heredoc_delimiter.take().is_some() {
                    malformed = true;
                }
                i += 1;
                if i < chars.len() && chars[i] == '|' {
                    i += 1;
                    tokens.push(ShellToken::Separator);
                } else {
                    tokens.push(ShellToken::Pipe);
                }
            }
            ';' => {
                if awaiting_heredoc_delimiter.take().is_some() {
                    malformed = true;
                }
                tokens.push(ShellToken::Separator);
                i += 1;
            }
            '&' => {
                if awaiting_heredoc_delimiter.take().is_some() {
                    malformed = true;
                }
                tokens.push(ShellToken::Separator);
                i += 1;
                if i < chars.len() && chars[i] == '&' {
                    i += 1;
                }
            }
            _ => {
                let mut value = String::new();
                let mut dynamic_prefix: Option<DynamicPrefix> = None;
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
                                        dynamic_prefix.get_or_insert_with(|| DynamicPrefix {
                                            literal: value.clone(),
                                            kind: DynamicKind::Expansion,
                                        });
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
                        '$' | '`' | '(' | ')' => {
                            dynamic_prefix.get_or_insert_with(|| DynamicPrefix {
                                literal: value.clone(),
                                kind: DynamicKind::Expansion,
                            });
                            value.push(c);
                            i += 1;
                        }
                        '*' | '?' | '[' | '{' | '}' => {
                            dynamic_prefix.get_or_insert_with(|| DynamicPrefix {
                                literal: value.clone(),
                                kind: DynamicKind::Glob,
                            });
                            value.push(c);
                            i += 1;
                        }
                        '~' if value.is_empty() => {
                            dynamic_prefix.get_or_insert_with(|| DynamicPrefix {
                                literal: value.clone(),
                                kind: DynamicKind::Expansion,
                            });
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
                    if let Some(strip_tabs) = awaiting_heredoc_delimiter.take() {
                        heredocs.push(Heredoc {
                            delimiter: value.clone(),
                            strip_tabs,
                        });
                    }
                    tokens.push(ShellToken::Word(ShellWord {
                        value,
                        dynamic: dynamic_prefix.is_some(),
                        dynamic_prefix,
                    }));
                }
                if malformed {
                    break;
                }
            }
        }
    }

    if awaiting_heredoc_delimiter.is_some() || !heredocs.is_empty() {
        malformed = true;
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
///
/// Matched by path component rather than by string so path rendering details do
/// not affect the result.
fn is_non_file_device(resolved: &Path) -> bool {
    let Ok(rest) = resolved.strip_prefix("/dev") else {
        return false;
    };
    let mut parts = rest.components().map(|c| c.as_os_str().to_str());
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Some("null" | "stdout" | "stderr")), None, None) => true,
        (Some(Some("fd")), Some(Some(n)), None) => {
            !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
        }
        _ => false,
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
    resolved_targets.push(artifact_path(&resolved));
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
#[path = "shell_targets_tests.rs"]
mod tests;
