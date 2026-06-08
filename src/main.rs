//! Thin binary entry point. All logic lives in the library crate (`eval_magic`)
//! so it stays unit-testable; this file only wires `main` to the CLI and maps
//! errors to a process exit code.

use std::process::ExitCode;

fn main() -> ExitCode {
    match eval_magic::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
