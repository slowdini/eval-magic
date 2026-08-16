//! The hidden `__fixture` subcommand: a predictable child process for the test
//! suite to spawn.
//!
//! Tests that exercise `command_check` grading need a program that exits with a
//! chosen status, emits chosen bytes, or writes a chosen file. Reaching for
//! `sh`, `true`, or `printf` ties those tests to POSIX, and the `cmd.exe`
//! equivalents are not equivalent — `echo x>>f` appends CRLF, and
//! `echo|set /p=` cannot round-trip a value. One fixture invoked the same way
//! under both shells removes the dialect problem entirely.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process;

use anyhow::Context;

use crate::cli::args::FixtureArgs;

/// Run the fixture and leave the process with its exit code. Streams are
/// flushed explicitly: `process::exit` does not run destructors, so buffered
/// output would otherwise be dropped.
pub(crate) fn run_fixture(args: FixtureArgs) -> anyhow::Result<()> {
    let mut out = io::stdout();
    let mut err = io::stderr();
    let code = execute_fixture(&args, &mut out, &mut err)?;
    out.flush()?;
    err.flush()?;
    process::exit(code)
}

/// Perform the fixture's effects against `out`/`err`, returning the exit code.
/// Split from [`run_fixture`] so tests can drive it with in-memory buffers.
fn execute_fixture(
    args: &FixtureArgs,
    out: &mut impl Write,
    err: &mut impl Write,
) -> anyhow::Result<i32> {
    let satisfied = requirements_met(args)?;
    let emitted = emitted_output(args);

    out.write_all(emitted.as_bytes())?;
    if let Some(text) = &args.stderr {
        err.write_all(text.as_bytes())?;
    }
    if let Some(path) = &args.write {
        write_to(path, &emitted, false)?;
    }
    if let Some(path) = &args.append {
        write_to(path, &emitted, true)?;
    }

    Ok(if satisfied { args.exit } else { 1 })
}

/// The single output string: `--pad`, then each `--text`, then each
/// `--echo-env`, joined by `--separator`. No site needs the two fragment kinds
/// interleaved, so a fixed order keeps the result independent of argv order —
/// which `clap` does not preserve across distinct flags.
fn emitted_output(args: &FixtureArgs) -> String {
    let mut fragments = Vec::new();
    if let Some(count) = args.pad {
        fragments.push("x".repeat(count));
    }
    fragments.extend(args.text.iter().cloned());
    fragments.extend(args.echo_env.iter().map(|name| {
        std::env::var(name).unwrap_or_else(|_| args.default.clone().unwrap_or_default())
    }));
    let mut emitted = fragments.join(&args.separator);
    if args.newline {
        emitted.push('\n');
    }
    emitted
}

/// Whether every `--require-*` check holds. Errors are reserved for the fixture
/// being unusable (an unreadable path, a malformed flag pairing); an unmet
/// requirement is an ordinary `false`, because that is the behavior under test.
fn requirements_met(args: &FixtureArgs) -> anyhow::Result<bool> {
    for path in &args.require_file {
        if !path.is_file() {
            return Ok(false);
        }
    }
    for pair in args.require_file_text.chunks(2) {
        let [path, expected] = pair else {
            anyhow::bail!("--require-file-text takes <path> <text>");
        };
        match fs::read_to_string(path) {
            Ok(actual) if &actual == expected => {}
            _ => return Ok(false),
        }
    }
    for spec in &args.require_env {
        let met = match spec.split_once('=') {
            Some((name, expected)) => std::env::var(name).is_ok_and(|value| value == expected),
            None => std::env::var(spec).is_ok_and(|value| !value.is_empty()),
        };
        if !met {
            return Ok(false);
        }
    }
    if let Some(paths) = &args.files_equal {
        let [left, right] = paths.as_slice() else {
            anyhow::bail!("--files-equal takes <left> <right>");
        };
        match (fs::read(left), fs::read(right)) {
            (Ok(left), Ok(right)) if left == right => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// Write `contents` to `path`, creating missing parents so a fixture can target
/// a directory the test has not made yet.
fn write_to(path: &Path, contents: &str, append: bool) -> anyhow::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating fixture output dir {}", parent.display()))?;
    }
    OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(path)
        .and_then(|mut file| file.write_all(contents.as_bytes()))
        .with_context(|| format!("writing fixture output {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn args() -> FixtureArgs {
        FixtureArgs::default()
    }

    /// Runs the fixture against in-memory streams and returns
    /// `(exit_code, stdout, stderr)`.
    fn run(args: &FixtureArgs) -> (i32, String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = execute_fixture(args, &mut out, &mut err).unwrap();
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn emits_nothing_and_exits_zero_by_default() {
        assert_eq!(run(&args()), (0, String::new(), String::new()));
    }

    #[test]
    fn exit_code_is_reported_when_every_requirement_holds() {
        let (code, _, _) = run(&FixtureArgs { exit: 3, ..args() });
        assert_eq!(code, 3);
    }

    #[test]
    fn text_and_stderr_fragments_reach_their_own_streams() {
        let (code, out, err) = run(&FixtureArgs {
            text: vec!["hello world".into()],
            stderr: Some("diagnostic".into()),
            ..args()
        });
        assert_eq!(
            (code, out, err),
            (0, "hello world".into(), "diagnostic".into())
        );
    }

    /// No trailing newline unless asked: `command-runs.txt` is compared byte for
    /// byte against `"x"`, so an implicit newline would break it.
    #[test]
    fn fragments_are_joined_by_the_separator_and_newline_is_opt_in() {
        let (_, out, _) = run(&FixtureArgs {
            echo_env: vec!["EVAL_MAGIC_UNSET_A".into(), "EVAL_MAGIC_UNSET_B".into()],
            default: Some("v".into()),
            separator: "|".into(),
            ..args()
        });
        assert_eq!(out, "v|v");

        let (_, terminated, _) = run(&FixtureArgs {
            text: vec!["x".into()],
            newline: true,
            ..args()
        });
        assert_eq!(terminated, "x\n");
    }

    #[test]
    fn pad_emits_its_byte_count_ahead_of_the_other_fragments() {
        let (_, out, _) = run(&FixtureArgs {
            pad: Some(3000),
            text: vec!["TAIL".into()],
            ..args()
        });
        assert_eq!(out.len(), 3004);
        assert!(out.ends_with("TAIL"));
        assert!(out.starts_with("xxx"));
    }

    #[test]
    fn echo_env_reads_the_real_environment_and_falls_back_to_the_default() {
        let path = std::env::var("PATH").unwrap();
        let (_, present, _) = run(&FixtureArgs {
            echo_env: vec!["PATH".into()],
            ..args()
        });
        assert_eq!(present, path);

        let (_, absent, _) = run(&FixtureArgs {
            echo_env: vec!["EVAL_MAGIC_DEFINITELY_UNSET".into()],
            default: Some("unset".into()),
            ..args()
        });
        assert_eq!(absent, "unset");
    }

    #[test]
    fn write_replaces_and_append_extends_the_target_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("nested").join("out.txt");

        run(&FixtureArgs {
            text: vec!["first".into()],
            write: Some(target.clone()),
            ..args()
        });
        assert_eq!(fs::read_to_string(&target).unwrap(), "first");

        run(&FixtureArgs {
            text: vec!["second".into()],
            write: Some(target.clone()),
            ..args()
        });
        assert_eq!(fs::read_to_string(&target).unwrap(), "second");

        run(&FixtureArgs {
            text: vec!["x".into()],
            append: Some(target.clone()),
            ..args()
        });
        run(&FixtureArgs {
            text: vec!["x".into()],
            append: Some(target.clone()),
            ..args()
        });
        assert_eq!(fs::read_to_string(&target).unwrap(), "secondxx");
    }

    #[test]
    fn require_env_checks_presence_for_a_bare_name_and_equality_for_a_pair() {
        let path = std::env::var("PATH").unwrap();
        for (spec, expected) in [
            ("PATH".to_string(), 0),
            ("EVAL_MAGIC_DEFINITELY_UNSET".to_string(), 1),
            (format!("PATH={path}"), 0),
            ("PATH=not-the-real-path".to_string(), 1),
        ] {
            let (code, _, _) = run(&FixtureArgs {
                require_env: vec![spec.clone()],
                ..args()
            });
            assert_eq!(code, expected, "{spec}");
        }
    }

    #[test]
    fn require_file_checks_existence() {
        let tmp = tempfile::TempDir::new().unwrap();
        let present = tmp.path().join("present.txt");
        fs::write(&present, "body").unwrap();

        let (ok, _, _) = run(&FixtureArgs {
            require_file: vec![present],
            ..args()
        });
        assert_eq!(ok, 0);

        let (missing, _, _) = run(&FixtureArgs {
            require_file: vec![tmp.path().join("absent.txt")],
            ..args()
        });
        assert_eq!(missing, 1);
    }

    #[test]
    fn require_file_text_compares_contents_to_a_literal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = tmp.path().join("state.txt");
        fs::write(&state, "ready").unwrap();

        let (ok, _, _) = run(&FixtureArgs {
            require_file_text: vec![state.to_string_lossy().into_owned(), "ready".into()],
            ..args()
        });
        assert_eq!(ok, 0);

        let (mismatch, _, _) = run(&FixtureArgs {
            require_file_text: vec![state.to_string_lossy().into_owned(), "waiting".into()],
            ..args()
        });
        assert_eq!(mismatch, 1);
    }

    #[test]
    fn files_equal_compares_two_files_byte_for_byte() {
        let tmp = tempfile::TempDir::new().unwrap();
        let answer = tmp.path().join("answer.txt");
        let expected = tmp.path().join("expected.txt");
        fs::write(&answer, "same").unwrap();
        fs::write(&expected, "same").unwrap();

        let (matching, _, _) = run(&FixtureArgs {
            files_equal: Some(vec![answer.clone(), expected.clone()]),
            ..args()
        });
        assert_eq!(matching, 0);

        fs::write(&expected, "different").unwrap();
        let (differing, _, _) = run(&FixtureArgs {
            files_equal: Some(vec![answer, expected]),
            ..args()
        });
        assert_eq!(differing, 1);
    }

    /// A failed requirement must not suppress the effects: the matrix suite
    /// reads `matrix-runs.txt` to prove every cell ran, including the cells that
    /// are expected to fail.
    #[test]
    fn a_failed_requirement_still_performs_the_effects() {
        let tmp = tempfile::TempDir::new().unwrap();
        let log = tmp.path().join("matrix-runs.txt");

        let (code, out, _) = run(&FixtureArgs {
            text: vec!["ran".into()],
            newline: true,
            append: Some(log.clone()),
            require_env: vec!["EVAL_MAGIC_DEFINITELY_UNSET".into()],
            exit: 0,
            ..args()
        });

        assert_eq!(code, 1);
        assert_eq!(out, "ran\n");
        assert_eq!(fs::read_to_string(&log).unwrap(), "ran\n");
    }

    /// A failed requirement outranks an explicit `--exit 0`, so a cell cannot
    /// report success while its precondition is unmet.
    #[test]
    fn a_failed_requirement_overrides_the_requested_exit_code() {
        let (code, _, _) = run(&FixtureArgs {
            exit: 0,
            require_env: vec!["EVAL_MAGIC_DEFINITELY_UNSET=value".into()],
            ..args()
        });
        assert_eq!(code, 1);
    }
}
