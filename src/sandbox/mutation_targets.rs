//! Target-aware classification for common development commands that mutate the filesystem.
//!
//! Package installs, pip installs, Cargo builds/tests, and in-place `sed` edits use the invocation
//! cwd as their implicit destination and validate the path options they own. The commands that
//! write a path outright — `touch`, `mkdir`, `rm`, `cp`, `mv`, `install` — live in [`filesystem`],
//! which drives them from one table. This stays narrower than a general shell parser; unrecognized
//! commands remain the post-hoc audit's responsibility.

use std::path::Path;

use crate::core::fs::artifact_path;

use super::policy::{BashClassification, is_under_any, resolve_path};
use super::shell_targets::{DynamicKind, ShellToken, ShellWord, lex_shell};

mod filesystem;

pub(super) use filesystem::is_mutator_reason;

const PACKAGE_REASON: &str = "package install/add";
const PIP_REASON: &str = "pip install";
const SED_REASON: &str = "in-place file edit (sed -i)";
const CARGO_REASON: &str = "cargo build/test output";

#[derive(Clone, Copy)]
struct PathOption {
    long: &'static str,
    short: Option<&'static str>,
    attached_short: bool,
}

fn is_command(word: &ShellWord, name: &str) -> bool {
    Path::new(&word.value)
        .file_name()
        .is_some_and(|file_name| file_name == name)
        && !word.dynamic
}

fn command_position(words: &[&ShellWord], names: &[&str]) -> Option<usize> {
    words
        .iter()
        .position(|word| names.iter().any(|name| is_command(word, name)))
}

fn has_word(words: &[&ShellWord], start: usize, values: &[&str]) -> bool {
    words
        .iter()
        .skip(start)
        .take_while(|word| word.value != "--")
        .any(|word| values.contains(&word.value.as_str()))
}

fn package_global_mode(words: &[&ShellWord], manager: &str, command: usize, action: usize) -> bool {
    for (index, word) in words.iter().enumerate().skip(command + 1) {
        if word.value == "--" {
            break;
        }
        match word.value.as_str() {
            "--global" | "-g" => return true,
            "global" if manager == "yarn" && index < action => return true,
            "--location" if manager == "npm" => {
                let Some(location) = words.get(index + 1) else {
                    return true;
                };
                if location.dynamic || location.value != "project" {
                    return true;
                }
            }
            value if value.starts_with("--global=") || value.starts_with("-g=") => {
                let enabled = value.split_once('=').map(|(_, value)| value).unwrap_or("");
                if word.dynamic || !matches!(enabled, "false" | "0") {
                    return true;
                }
            }
            value if manager == "npm" && value.starts_with("--location=") => {
                let location = value.strip_prefix("--location=").unwrap_or_default();
                if word.dynamic || location != "project" {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn denial(reason: &'static str, resolved_targets: Vec<String>) -> BashClassification {
    BashClassification {
        reason,
        resolved_targets,
    }
}

/// The one directory a dynamic target can expand within, when the expansion is
/// confined enough to name one.
///
/// A glob or brace draws its result from a directory listing, so a target whose
/// dynamic part is a single path component cannot land outside the directory
/// its literal prefix names. Rejecting a remainder that carries `/` is what
/// keeps that true — `target/*/../..` would otherwise walk back out — and
/// rejecting a component that starts with `.` keeps `.*`, which matches `..`,
/// from doing the same. A variable or command expansion names no directory at
/// all, so it never qualifies.
fn glob_directory(target: &ShellWord) -> Option<&str> {
    let prefix = target.dynamic_prefix.as_ref()?;
    if prefix.kind != DynamicKind::Glob {
        return None;
    }
    if target.value.get(prefix.literal.len()..)?.contains('/') {
        return None;
    }
    let (directory, component) = match prefix.literal.rfind('/') {
        Some(slash) => prefix.literal.split_at(slash + 1),
        // No directory in the prefix: the pattern expands within the cwd.
        None => (".", prefix.literal.as_str()),
    };
    (!component.starts_with('.')).then_some(directory)
}

fn target_denial(
    reason: &'static str,
    target: &ShellWord,
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    if target.value.is_empty() || target.value.contains('\0') {
        return Some(denial(reason, Vec::new()));
    }
    let spelling = if target.dynamic {
        // Nothing analyzable about the expansion: fail closed with no evidence,
        // rather than record a half-expanded spelling.
        let Some(directory) = glob_directory(target) else {
            return Some(denial(reason, Vec::new()));
        };
        directory
    } else {
        target.value.as_str()
    };
    if is_under_any(spelling, allowed_roots, invocation_cwd) {
        return None;
    }
    Some(denial(
        reason,
        vec![artifact_path(&resolve_path(spelling, invocation_cwd))],
    ))
}

fn cwd_denial(
    reason: &'static str,
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    target_denial(
        reason,
        &ShellWord::literal("."),
        allowed_roots,
        invocation_cwd,
    )
}

/// Validate every recognized path option. The boolean says whether the command
/// supplied at least one explicit destination.
fn validate_path_options(
    words: &[&ShellWord],
    start: usize,
    options: &[PathOption],
    reason: &'static str,
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Result<bool, BashClassification> {
    let mut saw_target = false;
    let mut index = start;
    while index < words.len() {
        let word = words[index];
        if word.value == "--" {
            break;
        }
        let mut matched = false;
        for option in options {
            let separate = word.value == option.long
                || option
                    .short
                    .is_some_and(|short| word.value.as_str() == short);
            if separate {
                saw_target = true;
                let Some(target) = words.get(index + 1) else {
                    return Err(denial(reason, Vec::new()));
                };
                if target.value.starts_with('-') {
                    return Err(denial(reason, Vec::new()));
                }
                if let Some(denial) = target_denial(reason, target, allowed_roots, invocation_cwd) {
                    return Err(denial);
                }
                index += 2;
                matched = true;
                break;
            }

            let long_prefix = format!("{}=", option.long);
            let inline = word.value.strip_prefix(&long_prefix).or_else(|| {
                option.short.and_then(|short| {
                    option
                        .attached_short
                        .then(|| word.value.strip_prefix(short))
                        .flatten()
                        .filter(|value| !value.is_empty())
                })
            });
            if let Some(value) = inline {
                saw_target = true;
                let target = ShellWord::slice_of(value, word);
                if let Some(denial) = target_denial(reason, &target, allowed_roots, invocation_cwd)
                {
                    return Err(denial);
                }
                index += 1;
                matched = true;
                break;
            }
        }
        if !matched {
            index += 1;
        }
    }
    Ok(saw_target)
}

fn classify_package_manager(
    words: &[&ShellWord],
    manager: &str,
    path_options: &[PathOption],
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    let command = command_position(words, &[manager])?;
    let action = words
        .iter()
        .enumerate()
        .skip(command + 1)
        .take_while(|(_, word)| word.value != "--")
        .find(|(_, word)| ["install", "add", "ci", "i"].contains(&word.value.as_str()))
        .map(|(index, _)| index)?;
    let saw_destination = match validate_path_options(
        words,
        command + 1,
        path_options,
        PACKAGE_REASON,
        allowed_roots,
        invocation_cwd,
    ) {
        Ok(saw_destination) => saw_destination,
        Err(denial) => return Some(denial),
    };
    let global = package_global_mode(words, manager, command, action);
    if global && !(manager == "npm" && saw_destination) {
        return Some(denial(PACKAGE_REASON, Vec::new()));
    }
    cwd_denial(PACKAGE_REASON, allowed_roots, invocation_cwd)
}

fn classify_package_install(
    words: &[&ShellWord],
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    const NPM: &[PathOption] = &[PathOption {
        long: "--prefix",
        short: None,
        attached_short: false,
    }];
    const PNPM: &[PathOption] = &[PathOption {
        long: "--dir",
        short: Some("-C"),
        attached_short: true,
    }];
    const YARN_BUN: &[PathOption] = &[PathOption {
        long: "--cwd",
        short: None,
        attached_short: false,
    }];

    [
        ("npm", NPM),
        ("pnpm", PNPM),
        ("yarn", YARN_BUN),
        ("bun", YARN_BUN),
    ]
    .into_iter()
    .find_map(|(manager, options)| {
        classify_package_manager(words, manager, options, allowed_roots, invocation_cwd)
    })
}

fn classify_pip_install(
    words: &[&ShellWord],
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    const OPTIONS: &[PathOption] = &[
        PathOption {
            long: "--target",
            short: Some("-t"),
            attached_short: false,
        },
        PathOption {
            long: "--prefix",
            short: None,
            attached_short: false,
        },
        PathOption {
            long: "--root",
            short: None,
            attached_short: false,
        },
        PathOption {
            long: "--src",
            short: None,
            attached_short: false,
        },
    ];

    let command = command_position(words, &["pip", "pip3"])?;
    if !has_word(words, command + 1, &["install"]) {
        return None;
    }
    if let Err(denial) = validate_path_options(
        words,
        command + 1,
        OPTIONS,
        PIP_REASON,
        allowed_roots,
        invocation_cwd,
    ) {
        return Some(denial);
    }
    if has_word(words, command + 1, &["--user"]) {
        return Some(denial(PIP_REASON, Vec::new()));
    }
    cwd_denial(PIP_REASON, allowed_roots, invocation_cwd)
}

fn assignment_target(word: &ShellWord, name: &str) -> Option<ShellWord> {
    word.value
        .strip_prefix(&format!("{name}="))
        .map(|value| ShellWord::slice_of(value, word))
}

fn classify_cargo(
    words: &[&ShellWord],
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    const OPTIONS: &[PathOption] = &[
        PathOption {
            long: "--target-dir",
            short: None,
            attached_short: false,
        },
        PathOption {
            long: "-C",
            short: None,
            attached_short: false,
        },
    ];

    let command = command_position(words, &["cargo"])?;
    if !has_word(words, command + 1, &["build", "test"]) {
        return None;
    }
    if let Some(denial) = cwd_denial(CARGO_REASON, allowed_roots, invocation_cwd) {
        return Some(denial);
    }
    for word in &words[..command] {
        for name in ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"] {
            if let Some(target) = assignment_target(word, name)
                && let Some(denial) =
                    target_denial(CARGO_REASON, &target, allowed_roots, invocation_cwd)
            {
                return Some(denial);
            }
        }
    }
    validate_path_options(
        words,
        command + 1,
        OPTIONS,
        CARGO_REASON,
        allowed_roots,
        invocation_cwd,
    )
    .err()
}

fn classify_sed(
    words: &[&ShellWord],
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    let command = command_position(words, &["sed"])?;
    let mut in_place = false;
    let mut expression_option = false;
    let mut positionals = Vec::new();
    let mut index = command + 1;

    while index < words.len() {
        let word = words[index];
        match word.value.as_str() {
            "-i" | "--in-place" => {
                in_place = true;
                if words
                    .get(index + 1)
                    .is_some_and(|next| next.value.is_empty())
                {
                    index += 1;
                }
            }
            "-e" | "--expression" | "-f" | "--file" => {
                expression_option = true;
                index += usize::from(words.get(index + 1).is_some());
            }
            value
                if value.starts_with("-i") && value.len() > 2
                    || value.starts_with("--in-place=") =>
            {
                in_place = true;
            }
            value if (value.starts_with("-e") || value.starts_with("-f")) && value.len() > 2 => {
                expression_option = true;
            }
            value if value.starts_with('-') => {}
            _ => positionals.push(word),
        }
        index += 1;
    }
    if !in_place {
        return None;
    }

    let targets = if expression_option {
        positionals.as_slice()
    } else {
        positionals.get(1..).unwrap_or_default()
    };
    targets
        .iter()
        .find_map(|target| target_denial(SED_REASON, target, allowed_roots, invocation_cwd))
}

fn classify_segment(
    words: &[&ShellWord],
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    classify_package_install(words, allowed_roots, invocation_cwd)
        .or_else(|| classify_pip_install(words, allowed_roots, invocation_cwd))
        .or_else(|| classify_cargo(words, allowed_roots, invocation_cwd))
        .or_else(|| classify_sed(words, allowed_roots, invocation_cwd))
        .or_else(|| filesystem::classify(words, allowed_roots, invocation_cwd))
}

/// Whether this segment is one of the development mutations whose in-bounds
/// execution still requires an explicit command-policy allowance.
pub(super) fn segment_is_recognized(words: &[ShellWord]) -> bool {
    let words: Vec<&ShellWord> = words.iter().collect();

    let package = ["npm", "pnpm", "yarn", "bun"].iter().any(|manager| {
        command_position(&words, &[*manager]).is_some_and(|command| {
            words
                .iter()
                .skip(command + 1)
                .take_while(|word| word.value != "--")
                .any(|word| ["install", "add", "ci", "i"].contains(&word.value.as_str()))
        })
    });
    let pip = command_position(&words, &["pip", "pip3"])
        .is_some_and(|command| has_word(&words, command + 1, &["install"]));
    let cargo = command_position(&words, &["cargo"])
        .is_some_and(|command| has_word(&words, command + 1, &["build", "test"]));
    let sed = command_position(&words, &["sed"]).is_some_and(|command| {
        words.iter().skip(command + 1).any(|word| {
            matches!(word.value.as_str(), "-i" | "--in-place")
                || word.value.starts_with("--in-place=")
                || word.value.starts_with("-i") && word.value.len() > 2
        })
    });

    package || pip || cargo || sed
}

pub(super) fn classify_mutation_targets(
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
                if let Some(denial) = classify_segment(&segment, allowed_roots, invocation_cwd) {
                    return Some(denial);
                }
                segment.clear();
            }
            ShellToken::OutputRedirect | ShellToken::FdDuplicate | ShellToken::InputRedirect => {}
        }
    }
    classify_segment(&segment, allowed_roots, invocation_cwd).or_else(|| {
        // A broken quote or escape can turn an explicit destination into a
        // misleading partial literal. Only fail closed when the malformed
        // segment is otherwise recognizable as one of the mutations this
        // module owns; unrelated malformed shell remains outside this
        // intentionally narrow heuristic.
        lexed
            .malformed
            .then(|| classify_segment(&segment, &[], invocation_cwd))
            .flatten()
            .map(|classification| denial(classification.reason, Vec::new()))
    })
}
