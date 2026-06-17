//! `validate` — schema-check every `evals.json` under `--skill-dir`.

use std::path::Path;

use anyhow::bail;

use crate::cli::args::ValidateArgs;
use crate::core::{DetectInput, detect_run_context};
use crate::validation;

/// Validate eval config files, printing a `✓`/`✗` line per file and a summary.
/// With `--skill-dir`, validates every child skill. Otherwise validates the
/// current skill directory or `--skill <path-or-name>`.
pub(crate) fn run_validate(args: ValidateArgs) -> anyhow::Result<()> {
    let report = if let Some(skill_dir) = args.skill_dir {
        let skill_dir = Path::new(&skill_dir);
        if !skill_dir.is_dir() {
            bail!("skills dir not found: {}", skill_dir.display());
        }
        validation::validate_all(skill_dir)?
    } else {
        let ctx = detect_run_context(DetectInput {
            skill: args.skill,
            ..Default::default()
        })?;
        validation::validate_one(&ctx.skill_subdir)?
    };
    for outcome in &report.outcomes {
        match &outcome.error {
            None => println!("✓ {}", outcome.rel_path),
            Some(message) => eprintln!("✗ {message}"),
        }
    }
    println!(
        "\nValidated {} evals.json file(s); {} failed.",
        report.validated(),
        report.failed()
    );

    if report.failed() > 0 {
        let details = report
            .outcomes
            .iter()
            .filter_map(|o| o.error.as_ref().map(|m| format!("  - {}: {m}", o.skill)))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "{} evals.json file(s) failed validation:\n{details}",
            report.failed()
        );
    }

    Ok(())
}
