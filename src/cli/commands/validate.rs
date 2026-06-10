//! `validate` — schema-check every `evals.json` under `--skill-dir`.

use std::path::Path;

use anyhow::bail;

use crate::cli::args::ValidateArgs;
use crate::validation;

/// Validate every `<skill>/evals/evals.json` under `--skill-dir`, printing a
/// `✓`/`✗` line per file and a summary. Exits non-zero if any file failed.
pub(crate) fn run_validate(args: ValidateArgs) -> anyhow::Result<()> {
    let skill_dir = args
        .skill_dir
        .ok_or_else(|| anyhow::anyhow!("missing required flag --skill-dir <path>"))?;
    let skill_dir = Path::new(&skill_dir);
    if !skill_dir.is_dir() {
        bail!("skills dir not found: {}", skill_dir.display());
    }

    let report = validation::validate_all(skill_dir)?;
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
