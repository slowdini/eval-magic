//! CLI surface: command-tree definition and dispatch.
//!
//! Mirrors `src/cli/` in eval-runner (`cli.ts`, `run.ts`, `help.ts`). The
//! manual flag parsing and hand-written help of the original are replaced by a
//! `clap` derive tree, so `help.ts` has no counterpart here.
//!
//! The full subcommand surface is fixed here up front; each handler reports
//! "not yet implemented" until its owning module is ported (see
//! rewrite-roadmap.md). Flags are intentionally permissive (mostly optional)
//! during the port and are tightened per-command as behavior lands.

use std::path::Path;

use anyhow::bail;
use clap::{Args, Parser, Subcommand};

use crate::validation;

/// Top-level CLI. With no subcommand, the default action is `run`.
#[derive(Debug, Parser)]
#[command(
    name = "skill-eval",
    version,
    about = "Run skill evals — measure whether an agent skill actually shifts behavior."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

/// Flags shared by most subcommands. Ported from the manual `flag()` parsing in
/// eval-runner's `run.ts`/`cli.ts`.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Directory containing the skill under evaluation.
    #[arg(long)]
    pub skill_dir: Option<String>,
    /// Skill name under evaluation.
    #[arg(long)]
    pub skill: Option<String>,
    /// Iteration number for post-dispatch steps.
    #[arg(long)]
    pub iteration: Option<u32>,
    /// Comparison mode: `new-skill` (with vs. without) or `revision` (old vs. new).
    #[arg(long)]
    pub mode: Option<String>,
    /// Target harness.
    #[arg(long)]
    pub harness: Option<String>,
    /// Workspace directory (defaults to `<cwd>/skills-workspace`).
    #[arg(long)]
    pub workspace_dir: Option<String>,
    /// Restrict to these eval ids (comma-separated).
    #[arg(long)]
    pub only: Option<String>,
    /// Skip these eval ids (comma-separated).
    #[arg(long)]
    pub skip: Option<String>,
    /// Replace existing records rather than erroring.
    #[arg(long)]
    pub overwrite: bool,
}

/// `validate` only needs to know where to look.
#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Directory whose `evals.json` files should be validated.
    #[arg(long)]
    pub skill_dir: Option<String>,
}

/// `grade` adds a finalize flag on top of the common set.
#[derive(Debug, Args)]
pub struct GradeArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Merge judge responses instead of emitting judge tasks.
    #[arg(long)]
    pub finalize: bool,
}

/// Every subcommand ported from eval-runner. Names match the original CLI.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Build dispatches and run evals (the default action).
    Run(CommonArgs),
    /// Snapshot a workspace baseline.
    Snapshot(CommonArgs),
    /// Tear down a workspace.
    Teardown(CommonArgs),
    /// Disarm the write guard.
    TeardownGuard(CommonArgs),
    /// Ingest recorded transcripts into run records.
    Ingest(CommonArgs),
    /// Finalize grading after judge responses are in.
    Finalize(CommonArgs),
    /// Assemble run records from a dispatch and its transcripts.
    RecordRuns(CommonArgs),
    /// Populate tool invocations from persisted transcripts.
    FillTranscripts(CommonArgs),
    /// Detect writes outside the sandbox output boundary.
    DetectStrayWrites(CommonArgs),
    /// Grade run records (transcript checks + LLM-judge task emission).
    Grade(GradeArgs),
    /// Aggregate before/after benchmark deltas.
    Aggregate(CommonArgs),
    /// Promote a benchmark + gradings into a committed baseline.
    PromoteBaseline(CommonArgs),
    /// Validate `evals.json` files against the bundled schemas.
    Validate(ValidateArgs),
}

/// Parse process arguments, dispatch to the selected subcommand, and return its
/// result. Called by the binary entry point.
pub fn run() -> anyhow::Result<()> {
    dispatch(Cli::parse().command)
}

fn dispatch(command: Option<Commands>) -> anyhow::Result<()> {
    // No subcommand means the default `run` action.
    let command = command.unwrap_or(Commands::Run(CommonArgs {
        skill_dir: None,
        skill: None,
        iteration: None,
        mode: None,
        harness: None,
        workspace_dir: None,
        only: None,
        skip: None,
        overwrite: false,
    }));

    match command {
        Commands::Validate(args) => run_validate(args),
        other => {
            let name = match other {
                Commands::Run(_) => "run",
                Commands::Snapshot(_) => "snapshot",
                Commands::Teardown(_) => "teardown",
                Commands::TeardownGuard(_) => "teardown-guard",
                Commands::Ingest(_) => "ingest",
                Commands::Finalize(_) => "finalize",
                Commands::RecordRuns(_) => "record-runs",
                Commands::FillTranscripts(_) => "fill-transcripts",
                Commands::DetectStrayWrites(_) => "detect-stray-writes",
                Commands::Grade(_) => "grade",
                Commands::Aggregate(_) => "aggregate",
                Commands::PromoteBaseline(_) => "promote-baseline",
                Commands::Validate(_) => unreachable!("handled above"),
            };
            bail!("`{name}` is not yet implemented (see rewrite-roadmap.md)");
        }
    }
}

/// Validate every `<skill>/evals/evals.json` under `--skill-dir`, printing a
/// `✓`/`✗` line per file and a summary. Exits non-zero if any file failed.
/// Ports eval-runner's `validate-all.ts` `main`.
fn run_validate(args: ValidateArgs) -> anyhow::Result<()> {
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
