//! Arguments and help text for scaffolding an eval suite.

use clap::Args;

/// `init` writes the first eval scaffold for a skill.
#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// Optional directory containing the skill under evaluation.
    ///
    /// Use this when the skill is an immediate child of a skills directory. If
    /// omitted, `init` uses `--skill <path-or-name>` or the current directory.
    /// `init` creates only the eval scaffold; it does not create the skill itself.
    #[arg(long)]
    pub skill_dir: Option<String>,
    /// Skill under evaluation.
    ///
    /// With `--skill-dir`, this is the child folder name, inferred when the
    /// directory contains exactly one skill. Without `--skill-dir`, this is a
    /// skill directory path, or a child directory name relative to the current
    /// directory. This value becomes the generated `skill_name`.
    #[arg(long)]
    pub skill: Option<String>,
    /// Stable kebab-case id for the first eval case.
    ///
    /// If omitted, prompts interactively. The id is used as the workspace eval
    /// directory name, so it must satisfy the eval schema's kebab-case pattern.
    #[arg(long)]
    pub id: Option<String>,
    /// User-facing prompt the eval subagent receives.
    ///
    /// If omitted, prompts interactively. Write this like a realistic user
    /// request, not like an instruction to satisfy the eval.
    #[arg(long)]
    pub prompt: Option<String>,
    /// Human-readable description of a successful response.
    ///
    /// If omitted, prompts interactively. This seeds `expected_output`; add
    /// concrete assertions after seeing iteration 1 outputs.
    #[arg(long = "expected-output")]
    pub expected_output: Option<String>,
    /// Whether the skill is expected to trigger for this eval.
    ///
    /// Defaults to true and is omitted from the generated JSON. Set false for
    /// negative evals where correct behavior is not invoking the skill.
    #[arg(long)]
    pub skill_should_trigger: Option<bool>,
    /// Git repository URL to use as the eval codebase.
    ///
    /// Requires `--codebase-ref`. `init` records both values without contacting
    /// the remote, so use a full commit SHA when the scaffold must be reproducible.
    /// Conflicts with `--codebase-path` and `--codebase-cwd`.
    #[arg(
        long,
        value_name = "URL",
        requires = "codebase_ref",
        conflicts_with_all = ["codebase_path", "codebase_cwd"]
    )]
    pub codebase_url: Option<String>,
    /// Git ref paired with `--codebase-url`.
    ///
    /// The ref is recorded as given without a remote lookup. A full commit SHA
    /// is the reproducible choice; branches and tags can move.
    #[arg(
        long,
        value_name = "REF",
        requires = "codebase_url",
        conflicts_with_all = ["codebase_path", "codebase_cwd"]
    )]
    pub codebase_ref: Option<String>,
    /// Local directory to use as the eval codebase.
    ///
    /// A relative path resolves from the invocation directory and is written
    /// relative to the generated `evals/` directory. An absolute path remains
    /// absolute. Both forms are canonicalized before writing.
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with_all = ["codebase_url", "codebase_ref", "codebase_cwd"]
    )]
    pub codebase_path: Option<String>,
    /// Use the invocation directory as the eval codebase.
    ///
    /// Writes a path relative to the generated `evals/` directory. Conflicts
    /// with `--codebase-url`, `--codebase-ref`, and `--codebase-path`.
    #[arg(
        long,
        conflicts_with_all = ["codebase_url", "codebase_ref", "codebase_path"]
    )]
    pub codebase_cwd: bool,
    /// Overwrite an existing `<skill>/evals/evals.json`.
    ///
    /// Refuses to overwrite existing evals by default and checks that before
    /// resolving a local codebase or prompting for seed fields.
    #[arg(long)]
    pub force: bool,
}
