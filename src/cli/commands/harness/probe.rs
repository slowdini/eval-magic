//! The `harness lint --probe` live dispatch check: render
//! `dispatch.exec_template` with a trivial prompt in a throwaway temp dir,
//! execute it, and verify its configured transcript parser recovers a final
//! response. Invokes the real harness CLI, so it is opt-in and never part of
//! standard CI checks.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;

use anyhow::{Context, bail};

use crate::adapters::cli_command::{
    render_agent_dispatch_command, render_cli_model_arg, shell_quote_arg,
};
use crate::adapters::descriptor::{HarnessDescriptor, subst};
use crate::core::{ShellOutcome, run_in_posix_shell};

/// Options carried from the parsed `--probe` flags into [`run_probe`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProbeOpts {
    /// Skip the interactive `y/N` confirm banner.
    pub yes: bool,
    /// Hard ceiling on the single exec template invocation.
    pub timeout: Duration,
}

impl ProbeOpts {
    /// Build the options from the parsed `Lint` flags, or return `None` when
    /// `--probe` wasn't passed. Keeps the flag→opts mapping beside the type it
    /// produces, out of the `harness` dispatcher.
    pub(crate) fn from_flags(probe: bool, yes: bool, probe_timeout: Option<u64>) -> Option<Self> {
        probe.then_some(ProbeOpts {
            yes,
            timeout: Duration::from_secs(probe_timeout.unwrap_or(300)),
        })
    }
}

/// Failures surfaced individually by the probe so `run_probe` can report each
/// as its own `✗` line.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProbeError {
    #[error("failed to spawn the harness command: {0}")]
    SpawnFailed(String),
    #[error("exec template exited with {0}")]
    ExecFailed(ExitStatus),
    #[error("exec template timed out after {0:?}")]
    Timeout(Duration),
    #[error("the descriptor declares no transcript parser")]
    TranscriptUnavailable,
    #[error("{0} is missing")]
    TranscriptMissing(String),
    #[error("{path} could not be parsed: {message}")]
    TranscriptUnreadable { path: String, message: String },
    #[error("{0} contains no non-empty final response")]
    FinalResponseMissing(String),
}

/// Render the exec template with the angle placeholders (`<eval-root>`,
/// `<dispatch_prompt_path>`, `<outputs_dir>`, `<round>`) shell-quoted and the
/// machine placeholders (`{model_arg}`, `{guard_args}`) filled. Mirrors the
/// conversation driver's single left-to-right pass; unknown braces pass
/// through verbatim. The probe is a single turn, so `<round>` resolves to `1`.
#[allow(clippy::needless_pass_by_value)]
fn render_probe_exec(
    template: &str,
    eval_root: &str,
    dispatch_prompt_path: &str,
    outputs_dir: &Path,
    model_arg: &str,
    guard_args: &str,
) -> String {
    let quoted_eval_root = shell_quote_arg(eval_root);
    let quoted_prompt_path = shell_quote_arg(dispatch_prompt_path);
    let quoted_outputs_dir = shell_quote_arg(&outputs_dir.to_string_lossy());
    subst(
        &template
            .replace("<eval-root>", &quoted_eval_root)
            .replace("<dispatch_prompt_path>", &quoted_prompt_path)
            .replace("<outputs_dir>", &quoted_outputs_dir)
            .replace("<round>", "1"),
        &[("model_arg", model_arg), ("guard_args", guard_args)],
    )
}

/// Verify the runner-readiness contract: the configured events file exists,
/// parses successfully, and yields a non-empty final response.
fn verify_transcript(descriptor: &HarnessDescriptor, outputs_dir: &Path) -> Result<(), ProbeError> {
    let transcript = descriptor
        .transcript
        .as_ref()
        .ok_or(ProbeError::TranscriptUnavailable)?;
    let path = outputs_dir.join(&transcript.events_filename);
    let display = path.display().to_string();
    if !path.exists() {
        return Err(ProbeError::TranscriptMissing(display));
    }
    let summary =
        transcript
            .parse_full(&path)
            .map_err(|error| ProbeError::TranscriptUnreadable {
                path: display.clone(),
                message: error.to_string(),
            })?;
    if summary
        .final_text
        .as_deref()
        .is_none_or(|text| text.trim().is_empty())
    {
        return Err(ProbeError::FinalResponseMissing(display));
    }
    Ok(())
}

/// The trivial prompt the probe dispatches: short, deterministic, and cheap.
const PROBE_PROMPT: &str = "Reply with the single word: ok\n";

pub(crate) fn run_probe(
    descriptor: HarnessDescriptor,
    target_display: &str,
    opts: ProbeOpts,
) -> anyhow::Result<()> {
    let label = descriptor.label.clone();
    let exec_template = match descriptor.dispatch.exec_template.as_deref() {
        Some(t) => t,
        None => {
            eprintln!("✗ no dispatch.exec_template — --probe has nothing to run");
            bail!("probe failed for {label}");
        }
    };
    // The probe never arms the guard, so {guard_args} resolves to the empty
    // fragment.
    let model_flag = descriptor.model.as_ref().map(|m| m.flag.as_str());
    let agent_env = descriptor.dispatch.env.clone();
    let model_arg = render_cli_model_arg(model_flag, None);
    let guard_args = "";

    // Throwaway eval_root: the subprocess runs from here, framework artifacts
    // land under <eval_root>/outputs. TempDir cleans up on drop.
    let eval_root_tmp = tempfile::TempDir::new().context("creating probe temp dir")?;
    let eval_root = eval_root_tmp.path();
    let outputs_dir = eval_root.join("outputs");
    std::fs::create_dir_all(&outputs_dir)
        .with_context(|| format!("creating probe outputs dir at {}", outputs_dir.display()))?;
    let prompt_path = eval_root.join("probe-prompt.md");
    std::fs::write(&prompt_path, PROBE_PROMPT)
        .with_context(|| format!("writing probe prompt at {}", prompt_path.display()))?;

    let eval_root_str = eval_root.to_string_lossy();
    let prompt_path_str = prompt_path.to_string_lossy();
    let command = render_agent_dispatch_command(
        &render_probe_exec(
            exec_template,
            &eval_root_str,
            &prompt_path_str,
            &outputs_dir,
            &model_arg,
            guard_args,
        ),
        &agent_env,
    );

    // Banner + confirm on stderr so it precedes the ✓/✗ result lines, which
    // follow the existing lint convention (✓ → stdout, ✗ → stderr).
    eprintln!();
    eprintln!("About to execute (harness {label}, target {target_display}):");
    eprintln!("  {command}");
    eprintln!("This invokes the real harness CLI (network, tokens, usage limits).");
    eprintln!("Timeout: {}s", opts.timeout.as_secs());
    if !opts.yes {
        eprint!("Proceed? [y/N] ");
        io::stderr().flush().ok();
        let mut line = String::new();
        // Default-deny on a non-TTY or any reply that isn't `y` — never let a
        // piped command spend usage on an unintended probe.
        if io::stdin().lock().read_line(&mut line).is_err()
            || !line.trim().eq_ignore_ascii_case("y")
        {
            eprintln!("aborted: --probe confirm declined (pass --yes to skip)");
            bail!("probe declined for {label}");
        }
    }

    let mut failed = 0u32;
    let probed = run_in_posix_shell(&command, eval_root, &BTreeMap::new(), Some(opts.timeout))
        .map_err(ProbeError::SpawnFailed);
    match probed {
        Ok(ShellOutcome::Exited(status)) if status.success() => {
            match verify_transcript(&descriptor, &outputs_dir) {
                Ok(()) => {
                    println!("✓ live exec template: transcript final response recovered")
                }
                Err(e) => {
                    eprintln!("✗ {e}");
                    failed += 1;
                }
            }
        }
        Ok(ShellOutcome::Exited(status)) => {
            eprintln!("✗ {}", ProbeError::ExecFailed(status));
            failed += 1;
        }
        Ok(ShellOutcome::TimedOut) => {
            eprintln!("✗ {}", ProbeError::Timeout(opts.timeout));
            failed += 1;
        }
        Err(e) => {
            eprintln!("✗ {e}");
            failed += 1;
        }
    }

    if failed > 0 {
        bail!("probe failed for {label}: {failed} check(s) failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dir(p: &str) -> PathBuf {
        PathBuf::from(p)
    }

    #[test]
    fn render_probe_exec_substitutes_angle_and_machine_placeholders() {
        let template = "agent --root <eval-root> --prompt <dispatch_prompt_path> \
                        --out <outputs_dir> --round <round> {model_arg} {guard_args}";
        let rendered = render_probe_exec(
            template,
            "/path with space/eval",
            "/path with space/probe-prompt.md",
            &dir("/var/tmp/out"),
            "--model-X gpt-x",
            "--guard on",
        );
        // Angle placeholders are shell-quoted because the values contain spaces.
        assert!(rendered.contains("--root '/path with space/eval'"));
        assert!(rendered.contains("--prompt '/path with space/probe-prompt.md'"));
        assert!(rendered.contains("--out /var/tmp/out"));
        assert!(rendered.contains("--round 1"));
        // Machine placeholders substituted in place.
        assert!(rendered.contains("--model-X gpt-x"));
        assert!(rendered.contains("--guard on"));
        assert!(!rendered.contains("{model_arg}"));
        assert!(!rendered.contains("{guard_args}"));
    }

    #[test]
    fn render_probe_exec_passes_shell_braces_through_verbatim() {
        let template = "xargs -I{} sh -c 'echo ${JOBS:-4} {model_arg}'";
        let rendered = render_probe_exec(template, "/e", "/p", &dir("/o"), "m", "g");
        assert!(rendered.contains("-I{}"));
        assert!(rendered.contains("${JOBS:-4}"));
        // The {model_arg} token inside the quoted echo becomes "m" with the
        // closing `'` immediately after (no trailing space supplied).
        assert!(rendered.contains("'echo ${JOBS:-4} m'"));
        assert!(!rendered.contains("{model_arg}"));
    }
}
