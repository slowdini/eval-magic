//! Arguments and help text for exploratory evidence comparison.

use clap::Args;

use super::args::CommonArgs;

pub(super) const ABOUT: &str =
    "Pair both conditions' evidence for exploratory, assertion-free review.";

pub(super) const LONG_ABOUT: &str = "Pair both conditions' evidence for exploratory, assertion-free review.

Reads the bounded `judge-evidence.md` files written by `ingest` for one eval and writes `iteration-N/compare/<eval-id>.md`. The report keeps both conditions together so a driving agent can inspect differences in the prompt, final message, code diff, changed files, conversation, and tool use before concrete assertions exist. It works when the eval declares no authored assertions. It is exploratory evidence, not a grade or a statistically reliable result.

Every run in a multi-run cell is included and labelled by run index. The command fails before replacing the report when either condition or any evidence bundle is missing. Run `eval-magic ingest` first; judge dispatch and finalization are not required. The report treats embedded content as untrusted read-only evidence and names available iteration-level validity reports. See `eval-magic docs judging` for the exploration workflow and evidence bounds.";

/// `compare` selects one eval on top of the shared workspace coordinates.
#[derive(Debug, Args)]
pub(crate) struct CompareArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Eval id whose two recorded conditions should be paired.
    #[arg(long, value_name = "ID")]
    pub eval: String,
}
