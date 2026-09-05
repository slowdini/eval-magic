use std::path::Path;

use super::{ShellToken, ShellWord, lex_shell};

/// Whether a shell command runs one of `read_commands` with `expected` as a
/// complete, literal argument. Literal wrapper scripts (for example
/// `zsh -lc "sed ... /path"`) are scanned recursively. Dynamic words and
/// output-redirection targets never count as deterministic access evidence.
pub(crate) fn command_reads_literal_path(
    command: &str,
    expected: &str,
    read_commands: &[String],
) -> bool {
    fn is_assignment(word: &ShellWord) -> bool {
        !word.dynamic
            && word
                .value
                .split_once('=')
                .is_some_and(|(name, _)| !name.is_empty() && !name.contains('/'))
    }

    fn segment_reads(words: &[ShellWord], expected: &str, read_commands: &[String]) -> bool {
        let Some(command_index) = words.iter().position(|word| !is_assignment(word)) else {
            return false;
        };
        let command = &words[command_index];
        if command.dynamic {
            return false;
        }
        let Some(name) = Path::new(&command.value)
            .file_name()
            .and_then(|name| name.to_str())
        else {
            return false;
        };
        read_commands.iter().any(|allowed| allowed == name)
            && words[command_index + 1..].iter().any(|word| {
                !word.dynamic && crate::core::fs::normalize_separators(&word.value) == expected
            })
    }

    fn literal_wrapper_script(words: &[ShellWord]) -> Option<&str> {
        let command_index = words.iter().position(|word| !is_assignment(word))?;
        let command = &words[command_index];
        if command.dynamic {
            return None;
        }
        let name = Path::new(&command.value).file_name()?.to_str()?;
        if !matches!(name, "sh" | "bash" | "dash" | "ksh" | "zsh") {
            return None;
        }

        let args = &words[command_index + 1..];
        let command_option = args.iter().position(|word| {
            !word.dynamic
                && word.value.starts_with('-')
                && word.value != "--"
                && word.value[1..].contains('c')
        })?;
        let script = args.get(command_option + 1)?;
        (!script.dynamic).then_some(script.value.as_str())
    }

    fn scan(command: &str, expected: &str, read_commands: &[String], depth: usize) -> bool {
        let lexed = lex_shell(command);
        if lexed.malformed {
            return false;
        }
        let mut segment = Vec::new();
        let mut skip_output_target = false;
        for token in lexed.tokens {
            match token {
                ShellToken::Word(word) => {
                    if skip_output_target {
                        skip_output_target = false;
                        continue;
                    }
                    segment.push(word);
                }
                // One native exit code cannot establish which member of a
                // compound command succeeded, so deterministic evidence is
                // limited to a single command segment.
                ShellToken::Pipe | ShellToken::Separator => return false,
                ShellToken::OutputRedirect => skip_output_target = true,
                ShellToken::FdDuplicate | ShellToken::InputRedirect => {}
            }
        }
        segment_reads(&segment, expected, read_commands)
            || depth < 2
                && literal_wrapper_script(&segment)
                    .is_some_and(|script| scan(script, expected, read_commands, depth + 1))
    }

    let expected = crate::core::fs::normalize_separators(expected);
    scan(command, &expected, read_commands, 0)
}
