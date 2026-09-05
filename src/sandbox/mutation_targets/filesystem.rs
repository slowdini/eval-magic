//! Target-aware classification for the shell commands that create, copy, move,
//! and delete files outright.
//!
//! Each family is one [`MutatorSpec`] entry and every entry is read by the same
//! walker, so teaching the guard another mutator is a table row rather than
//! another hand-written classifier. Deliberately narrower than a shell parser:
//! scripts, interpreters, and shell functions stay the post-hoc audit's
//! responsibility.

use std::path::Path;

use crate::sandbox::command_policy::is_assignment;
use crate::sandbox::policy::BashClassification;
use crate::sandbox::shell_targets::ShellWord;

use super::{is_command, target_denial};

const TOUCH_REASON: &str = "file creation (touch)";
const MKDIR_REASON: &str = "directory creation (mkdir)";
const RM_REASON: &str = "file removal (rm)";
const CP_REASON: &str = "file copy (cp)";
const MV_REASON: &str = "file move (mv)";
const INSTALL_REASON: &str = "file install (install)";

/// One option a mutator understands, matched in every spelling getopt accepts:
/// `--long value`, `--long=value`, `-s value`, `-svalue`, and `-s` inside a
/// bundled short cluster.
#[derive(Clone, Copy)]
struct MutatorOption {
    long: &'static str,
    short: Option<char>,
}

/// One filesystem-mutating command family, described declaratively.
///
/// The walker below is shared, so teaching the guard a new mutator is a table
/// entry rather than another hand-written classifier. Coreutils spell their
/// destinations in only a handful of shapes, and this records which shape each
/// command uses.
struct MutatorSpec {
    /// Executable basenames this spec claims.
    names: &'static [&'static str],
    reason: &'static str,
    /// Options whose argument is *required*, and which therefore carry an
    /// operand rather than a mutation target. Options with an *optional*
    /// argument never belong here: getopt reads those only from the
    /// `--long=value` spelling, so listing one would swallow the word after it.
    operand_options: &'static [MutatorOption],
    /// Which positionals name something the command writes.
    operands: OperandRole,
    /// A `-t DIR` style option naming the destination directory, which makes
    /// every positional a source.
    destination_option: Option<MutatorOption>,
    /// A switch that turns every positional into a directory the command
    /// creates: `install -d`.
    directory_switch: Option<MutatorOption>,
    /// The command removes its sources as well as writing its destination, so
    /// the sources are mutation targets too: `mv`.
    sources_mutated: bool,
}

enum OperandRole {
    /// Every positional is written: `touch`, `mkdir`, `rm`.
    All,
    /// The last positional is the destination and the rest are sources:
    /// `cp`, `mv`, `install`.
    LastIsDestination,
}

const TARGET_DIRECTORY: MutatorOption = MutatorOption {
    long: "--target-directory",
    short: Some('t'),
};

const SUFFIX: MutatorOption = MutatorOption {
    long: "--suffix",
    short: Some('S'),
};

const MUTATORS: &[MutatorSpec] = &[
    MutatorSpec {
        names: &["touch"],
        reason: TOUCH_REASON,
        // `-t` is a timestamp here, not a target directory.
        operand_options: &[
            MutatorOption {
                long: "--reference",
                short: Some('r'),
            },
            MutatorOption {
                long: "--date",
                short: Some('d'),
            },
            MutatorOption {
                long: "--time",
                short: Some('t'),
            },
        ],
        operands: OperandRole::All,
        destination_option: None,
        directory_switch: None,
        sources_mutated: false,
    },
    MutatorSpec {
        names: &["mkdir"],
        reason: MKDIR_REASON,
        operand_options: &[MutatorOption {
            long: "--mode",
            short: Some('m'),
        }],
        operands: OperandRole::All,
        destination_option: None,
        directory_switch: None,
        sources_mutated: false,
    },
    MutatorSpec {
        names: &["rm"],
        reason: RM_REASON,
        // `--interactive[=WHEN]` and `--preserve-root[=all]` take optional
        // arguments, which getopt never reads from the following word.
        operand_options: &[],
        operands: OperandRole::All,
        destination_option: None,
        directory_switch: None,
        sources_mutated: false,
    },
    MutatorSpec {
        names: &["cp"],
        reason: CP_REASON,
        // `--backup`, `--preserve`, `--reflink`, and `--context` all take
        // optional arguments and so consume no following word.
        operand_options: &[
            SUFFIX,
            MutatorOption {
                long: "--sparse",
                short: None,
            },
        ],
        operands: OperandRole::LastIsDestination,
        destination_option: Some(TARGET_DIRECTORY),
        directory_switch: None,
        sources_mutated: false,
    },
    MutatorSpec {
        names: &["mv"],
        reason: MV_REASON,
        operand_options: &[SUFFIX],
        operands: OperandRole::LastIsDestination,
        destination_option: Some(TARGET_DIRECTORY),
        directory_switch: None,
        // A move unlinks what it moved, so its sources are written too.
        sources_mutated: true,
    },
    MutatorSpec {
        names: &["install"],
        reason: INSTALL_REASON,
        operand_options: &[
            SUFFIX,
            MutatorOption {
                long: "--mode",
                short: Some('m'),
            },
            MutatorOption {
                long: "--owner",
                short: Some('o'),
            },
            MutatorOption {
                long: "--group",
                short: Some('g'),
            },
            MutatorOption {
                long: "--strip-program",
                short: None,
            },
        ],
        operands: OperandRole::LastIsDestination,
        destination_option: Some(TARGET_DIRECTORY),
        directory_switch: Some(MutatorOption {
            long: "--directory",
            short: Some('d'),
        }),
        sources_mutated: false,
    },
];

/// Whether `reason` names one of the filesystem mutators above.
pub(in crate::sandbox) fn is_mutator_reason(reason: &str) -> bool {
    MUTATORS.iter().any(|spec| spec.reason == reason)
}

/// A command that runs another command, described the same declarative way as
/// [`MutatorSpec`] so the wrapped executable stays visible.
struct WrapperSpec {
    name: &'static str,
    /// Options whose *next* word is their operand.
    operand_options: &'static [&'static str],
    /// Words the wrapper consumes after its options and before the command it
    /// runs — `timeout`'s duration.
    leading_operands: usize,
    /// The wrapper accepts `NAME=value` assignments before the command.
    accepts_assignments: bool,
}

const WRAPPERS: &[WrapperSpec] = &[
    WrapperSpec {
        name: "sudo",
        operand_options: &[
            "-u",
            "--user",
            "-g",
            "--group",
            "-p",
            "--prompt",
            "-C",
            "--close-from",
            "-D",
            "--chdir",
            "-h",
            "--host",
            "-U",
            "--other-user",
            "-r",
            "--role",
            "-t",
            "--type",
        ],
        leading_operands: 0,
        accepts_assignments: true,
    },
    WrapperSpec {
        name: "env",
        operand_options: &["-u", "--unset", "-C", "--chdir"],
        leading_operands: 0,
        accepts_assignments: true,
    },
    WrapperSpec {
        name: "command",
        operand_options: &[],
        leading_operands: 0,
        accepts_assignments: false,
    },
    WrapperSpec {
        name: "exec",
        operand_options: &["-a"],
        leading_operands: 0,
        accepts_assignments: false,
    },
    WrapperSpec {
        name: "nohup",
        operand_options: &[],
        leading_operands: 0,
        accepts_assignments: false,
    },
    WrapperSpec {
        name: "nice",
        operand_options: &["-n", "--adjustment"],
        leading_operands: 0,
        accepts_assignments: false,
    },
    WrapperSpec {
        name: "time",
        operand_options: &["-o", "--output", "-f", "--format"],
        leading_operands: 0,
        accepts_assignments: false,
    },
    WrapperSpec {
        name: "timeout",
        operand_options: &["-s", "--signal", "-k", "--kill-after"],
        leading_operands: 1,
        accepts_assignments: false,
    },
    WrapperSpec {
        name: "xargs",
        operand_options: &[
            "-I",
            "-i",
            "--replace",
            "-n",
            "--max-args",
            "-L",
            "-l",
            "--max-lines",
            "-P",
            "--max-procs",
            "-s",
            "--max-chars",
            "-d",
            "--delimiter",
            "-E",
            "-e",
            "--eof",
            "-a",
            "--arg-file",
            "--process-slot-var",
        ],
        leading_operands: 0,
        accepts_assignments: false,
    },
];

/// Step past leading assignments and any chain of wrappers to the index of the
/// executable the segment actually runs.
fn executable_index(words: &[&ShellWord]) -> Option<usize> {
    let mut index = words.iter().position(|word| !is_assignment(word))?;
    while let Some(wrapper) = WRAPPERS
        .iter()
        .find(|wrapper| is_command(words[index], wrapper.name))
    {
        index += 1;
        while let Some(word) = words.get(index) {
            if word.value == "--" {
                index += 1;
                break;
            }
            if wrapper.accepts_assignments && is_assignment(word) {
                index += 1;
                continue;
            }
            if word.value.starts_with('-') && word.value != "-" {
                index += 1 + usize::from(wrapper.operand_options.contains(&word.value.as_str()));
                continue;
            }
            break;
        }
        index += wrapper.leading_operands;
        if index >= words.len() {
            return None;
        }
    }
    Some(index)
}

/// The index of the executable this spec claims, or `None` when the segment
/// runs something else.
///
/// Unlike [`command_position`], which matches a name anywhere in the segment,
/// this anchors on the word actually being executed. The mutator names are
/// ordinary words that appear as arguments elsewhere: `npm install` would
/// otherwise read as coreutils `install`, and `echo mkdir /outside` as a
/// directory creation.
fn mutator_command_index(words: &[&ShellWord], names: &[&str]) -> Option<usize> {
    let index = executable_index(words)?;
    names
        .iter()
        .any(|name| is_command(words[index], name))
        .then_some(index)
}

/// Where a matched long option's value lives.
enum LongValue<'a> {
    /// Attached to the same word: `--opt=value`.
    Attached(&'a str),
    /// In the following word: `--opt value`.
    NextWord,
}

/// Match one word against one option's long spelling.
fn long_option<'a>(word: &'a str, option: &MutatorOption) -> Option<LongValue<'a>> {
    if let Some(value) = word
        .strip_prefix(option.long)
        .and_then(|rest| rest.strip_prefix('='))
    {
        return Some(LongValue::Attached(value));
    }
    (word == option.long).then_some(LongValue::NextWord)
}

/// What one mutator invocation names: the positionals following the executable,
/// plus the destination and the role its options selected.
struct MutatorOperands<'a> {
    positionals: Vec<&'a ShellWord>,
    /// The value of the spec's `-t`-style option, when one was supplied.
    destination: Option<ShellWord>,
    /// The spec's directory switch was given, so every positional is created.
    all_operands: bool,
}

/// Split one mutator invocation into its operands.
///
/// Bundled short clusters are walked left to right the way getopt walks them: a
/// short option whose argument is required takes the rest of its cluster as
/// that argument (`-m755`, `-tr`), or the following word when the cluster ends
/// at it (`-rt DIR`, `-Dm 755`). Reading them any other way would let a mode or
/// a suffix swallow the destination.
fn mutator_operands<'a>(
    spec: &MutatorSpec,
    words: &[&'a ShellWord],
    command: usize,
) -> MutatorOperands<'a> {
    let mut operands = MutatorOperands {
        positionals: Vec::new(),
        destination: None,
        all_operands: false,
    };
    let mut options = true;
    let mut index = command + 1;

    while index < words.len() {
        let word = words[index];
        if options && word.value == "--" {
            options = false;
            index += 1;
            continue;
        }
        if !options || word.dynamic || !word.value.starts_with('-') || word.value == "-" {
            operands.positionals.push(word);
            index += 1;
            continue;
        }
        index += 1;

        if word.value.starts_with("--") {
            if let Some(option) = spec.destination_option
                && let Some(found) = long_option(&word.value, &option)
            {
                match found {
                    LongValue::Attached(value) => {
                        operands.destination = Some(ShellWord::literal(value));
                    }
                    LongValue::NextWord => {
                        operands.destination = words.get(index).map(|next| (*next).clone());
                        index += 1;
                    }
                }
                continue;
            }
            if let Some(option) = spec.directory_switch
                && long_option(&word.value, &option).is_some()
            {
                operands.all_operands = true;
                continue;
            }
            if spec
                .operand_options
                .iter()
                .any(|option| matches!(long_option(&word.value, option), Some(LongValue::NextWord)))
            {
                index += 1;
            }
            continue;
        }

        let cluster = &word.value[1..];
        for (offset, letter) in cluster.char_indices() {
            let rest = &cluster[offset + letter.len_utf8()..];
            if spec
                .destination_option
                .is_some_and(|option| option.short == Some(letter))
            {
                if rest.is_empty() {
                    operands.destination = words.get(index).map(|next| (*next).clone());
                    index += 1;
                } else {
                    operands.destination = Some(ShellWord::literal(rest));
                }
                break;
            }
            if spec
                .operand_options
                .iter()
                .any(|option| option.short == Some(letter))
            {
                index += usize::from(rest.is_empty());
                break;
            }
            if spec
                .directory_switch
                .is_some_and(|option| option.short == Some(letter))
            {
                operands.all_operands = true;
            }
        }
    }
    operands
}

fn classify_mutator(
    spec: &MutatorSpec,
    words: &[&ShellWord],
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    let command = mutator_command_index(words, spec.names)?;
    let operands = mutator_operands(spec, words, command);
    let deny =
        |target: &ShellWord| target_denial(spec.reason, target, allowed_roots, invocation_cwd);

    let every_operand_is_written =
        operands.all_operands || matches!(spec.operands, OperandRole::All);
    let (destination, sources): (Option<&ShellWord>, &[&ShellWord]) = match &operands.destination {
        Some(destination) => (Some(destination), &operands.positionals),
        None if every_operand_is_written => {
            return operands.positionals.iter().find_map(|target| deny(target));
        }
        // With a single positional the command is a usage error; reading it as
        // the destination is the conservative choice.
        None => match operands.positionals.split_last() {
            Some((destination, sources)) => (Some(*destination), sources),
            None => (None, &[]),
        },
    };

    destination.and_then(deny).or_else(|| {
        spec.sources_mutated
            .then(|| sources.iter().find_map(|source| deny(source)))
            .flatten()
    })
}

/// The first denial any recognized filesystem mutation in this segment earns.
pub(super) fn classify(
    words: &[&ShellWord],
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    MUTATORS
        .iter()
        .find_map(|spec| classify_mutator(spec, words, allowed_roots, invocation_cwd))
}

#[cfg(test)]
#[path = "filesystem_tests.rs"]
mod tests;
