//! The `harness lint --probe` live dispatch check: render
//! `dispatch.exec_template` with a trivial prompt in a throwaway temp dir,
//! execute it, and verify `outputs/final-message.md` is recovered; also
//! render-only-validate `parallel_command_template` /
//! `judge_command_template` for placeholder-shape errors. Invokes the real
//! harness CLI, so it is opt-in and never part of standard CI checks.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use regex::Regex;

use crate::adapters::cli_command::{render_cli_model_arg, shell_quote_arg};
use crate::adapters::descriptor::{HarnessDescriptor, subst};

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
    #[error("outputs/final-message.md is missing")]
    FinalMessageMissing,
    #[error("outputs/final-message.md is empty")]
    FinalMessageEmpty,
    #[error("unresolved placeholder {0:?} remains after substitution")]
    UnresolvedBrace(String),
}

/// Render the exec template with the angle placeholders (`<eval-root>`,
/// `<dispatch_prompt_path>`, `<outputs_dir>`, `<round>`) shell-quoted and the
/// machine placeholders (`{model_arg}`, `{guard_args}`) filled. Mirrors the
/// conversation driver at `src/cli/run/conversation.rs:317` — single
/// left-to-right pass, unknown braces pass through verbatim. `<round>` is
/// intentionally not substituted: the probe is single-shot (turn 1 only), so
/// the exec template never sees rounds and any `<round>` survives verbatim.
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
            .replace("<outputs_dir>", &quoted_outputs_dir),
        &[("model_arg", model_arg), ("guard_args", guard_args)],
    )
}

/// Execute `command` via `/bin/sh -c` with `cwd` as the subprocess working
/// directory, killing the child if it exceeds `timeout`. The child's stdin is
/// `null`: the parent reads the `y/N` confirm on its own stdin and never wants
/// the dispatched agent CLI to consume it.
fn execute_with_timeout(
    command: &str,
    cwd: &Path,
    timeout: Duration,
) -> Result<ExitStatus, ProbeError> {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| ProbeError::SpawnFailed(e.to_string()))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProbeError::Timeout(timeout));
                }
                sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(ProbeError::SpawnFailed(e.to_string())),
        }
    }
}

/// Verify the final-message recovery contract: `outputs_dir/final-message.md`
/// exists and is non-empty after trimming.
fn verify_final_message(outputs_dir: &Path) -> Result<(), ProbeError> {
    let path = outputs_dir.join("final-message.md");
    if !path.exists() {
        return Err(ProbeError::FinalMessageMissing);
    }
    let contents = std::fs::read_to_string(&path).map_err(|_| ProbeError::FinalMessageMissing)?;
    if contents.trim().is_empty() {
        return Err(ProbeError::FinalMessageEmpty);
    }
    Ok(())
}

/// Render-only validate `template`: substitute the supplied stand-in `{vars}`,
/// then fail if any `{alpha_token}` placeholder remains unresolved — anywhere
/// in the rendered text, including embedded mid-token. Catches typos like
/// `{cwdd}` before a real run exercises the template. Shell idioms that are
/// not placeholder tokens (`${JOBS:-4}`, `-I{}`) do not match the pattern and
/// pass through cleanly.
fn render_only_check(template: &str, vars: &[(&str, &str)]) -> Result<(), ProbeError> {
    let rendered = subst(template, vars);
    static PLACEHOLDER: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = PLACEHOLDER
        .get_or_init(|| Regex::new(r"\{[a-zA-Z_][a-zA-Z0-9_]*\}").expect("placeholder regex"));
    if let Some(m) = re.find(&rendered) {
        return Err(ProbeError::UnresolvedBrace(m.as_str().to_string()));
    }
    Ok(())
}

/// Trivial prompt body written to `<eval_root>/probe-prompt.md`. The probe does
/// not assert on reply content — only that `outputs/final-message.md` ends up
/// non-empty — so the prompt is intentionally minimal and generic across every
/// harness.
const PROBE_PROMPT: &str = "Reply with the single word: ok\n";

/// Stand-in `{var}` values used by the render-only checks so a missing backing
/// field never masquerades as a clean render.
const RENDER_STAND_INS: [(&str, &str); 2] = [
    ("cwd", "/probe/stand-in/cwd"),
    ("model_arg", "stand-in-model"),
];

/// The live dispatch probe. Renders `dispatch.exec_template` with a trivial
/// prompt in a throwaway temp dir, asks for confirmation, runs it under a
/// timeout through `/bin/sh -c` from that dir, then verifies the final-message
/// recovery contract. Also render-only-validates `parallel_command_template`
/// and `judge_command_template` for placeholder-shape errors. Invokes the real
/// harness CLI and is opt-in; never part of standard CI checks.
///
/// `target_display` is the provenance string `lint` already printed under
/// `"Linted …"` (a file path or the joined layer chain); it appears only in the
/// confirm banner so the operator can see which descriptor is about to run.
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
    let parallel_template = descriptor.dispatch.parallel_command_template.clone();
    let judge_template = descriptor.dispatch.judge_command_template.clone();
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
    let command = render_probe_exec(
        exec_template,
        &eval_root_str,
        &prompt_path_str,
        &outputs_dir,
        &model_arg,
        guard_args,
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
    match execute_with_timeout(&command, eval_root, opts.timeout) {
        Ok(status) if status.success() => match verify_final_message(&outputs_dir) {
            Ok(()) => println!("✓ live exec template: final-message recovered"),
            Err(e) => {
                eprintln!("✗ {e}");
                failed += 1;
            }
        },
        Ok(status) => {
            eprintln!("✗ {}", ProbeError::ExecFailed(status));
            failed += 1;
        }
        Err(e) => {
            eprintln!("✗ {e}");
            failed += 1;
        }
    }

    // Render-only static checks: render each recipe with stand-in vars and fail
    // on any `{token}` the run would later surface. Catches typos like
    // `{cwdd}` before a real dispatch spends usage.
    if let Some(template) = parallel_template.as_deref() {
        match render_only_check(template, &RENDER_STAND_INS) {
            Ok(()) => println!("✓ render: parallel_command_template"),
            Err(e) => {
                eprintln!("✗ render: parallel_command_template: {e}");
                failed += 1;
            }
        }
    } else {
        println!("· parallel_command_template not declared — skipped");
    }
    if let Some(template) = judge_template.as_deref() {
        match render_only_check(template, &RENDER_STAND_INS) {
            Ok(()) => println!("✓ render: judge_command_template"),
            Err(e) => {
                eprintln!("✗ render: judge_command_template: {e}");
                failed += 1;
            }
        }
    } else {
        println!("· judge_command_template not declared — skipped");
    }

    if failed > 0 {
        bail!("probe failed for {label}: {failed} check(s) failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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
        // The probe is single-shot — `<round>` is not a probe placeholder and
        // must survive verbatim (the exec template never sees rounds).
        assert!(rendered.contains("--round <round>"));
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

    #[test]
    fn execute_with_timeout_returns_status_on_success() {
        let status = execute_with_timeout("true", Path::new("."), Duration::from_secs(5))
            .expect("true should succeed");
        assert!(status.success());
    }

    #[test]
    fn execute_with_timeout_kills_on_overrun() {
        let err = execute_with_timeout("sleep 5", Path::new("."), Duration::from_millis(100))
            .expect_err("sleep should time out");
        assert!(
            matches!(err, ProbeError::Timeout(d) if d == Duration::from_millis(100)),
            "got {err:?}"
        );
    }

    #[test]
    fn verify_final_message_accepts_a_non_empty_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("final-message.md"), "ok\n").unwrap();
        verify_final_message(tmp.path()).expect("non-empty file should pass");
    }

    #[test]
    fn verify_final_message_rejects_a_missing_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = verify_final_message(tmp.path()).expect_err("missing should fail");
        assert!(
            matches!(err, ProbeError::FinalMessageMissing),
            "got {err:?}"
        );
    }

    #[test]
    fn verify_final_message_rejects_a_blank_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("final-message.md"), "   \n\t \n").unwrap();
        let err = verify_final_message(tmp.path()).expect_err("blank should fail");
        assert!(matches!(err, ProbeError::FinalMessageEmpty), "got {err:?}");
    }

    #[test]
    fn render_only_check_passes_a_resolved_template() {
        let template = "judge --cd {cwd} $model_arg";
        let vars = [("cwd", "/work"), ("model_arg", "gpt-x")];
        render_only_check(template, &vars).expect("fully resolved template should pass");
    }

    #[test]
    fn render_only_check_fails_on_an_unresolved_brace() {
        let template = "judge --cd {cwd} --model {cwdd}";
        let vars = [("cwd", "/work"), ("model_arg", "gpt-x")];
        let err = render_only_check(template, &vars).expect_err("typo should fail");
        assert!(
            matches!(err, ProbeError::UnresolvedBrace(ref t) if t == "{cwdd}"),
            "got {err:?}"
        );
    }

    #[test]
    fn render_only_check_fails_on_a_brace_embedded_in_a_token() {
        // `--out {cwd}/final` substitutes {cwd} cleanly, but `--out {cwdd}/final`
        // (typo) leaves {cwdd} embedded mid-token — the check must still catch
        // it, not only standalone {cwdd}.
        let template = "agent --out {cwdd}/final";
        let vars = [("cwd", "/work"), ("model_arg", "gpt-x")];
        let err = render_only_check(template, &vars).expect_err("embedded typo should fail");
        assert!(
            matches!(err, ProbeError::UnresolvedBrace(ref t) if t == "{cwdd}"),
            "got {err:?}"
        );
    }

    #[test]
    fn render_only_check_passes_shell_brace_tokens_through() {
        // `${JOBS:-4}` and `-I{}` are real shell idioms the runbook uses; they
        // must not be mistaken for unresolved placeholders.
        let template = "xargs -I{} sh -c 'echo ${JOBS:-4}' {cwd}";
        let vars = [("cwd", "/work"), ("model_arg", "gpt-x")];
        render_only_check(template, &vars).expect("shell braces plus a resolved {cwd} should pass");
    }
}
