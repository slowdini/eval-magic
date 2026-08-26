//! Hermetic coverage for the opt-in live dispatch probe.

use super::*;

/// Descriptor with a fake `exec_template` that writes a parseable transcript.
const PROBE_OK_TOML: &str = r#"label = "probe-ok"

[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "probe-events.jsonl"
parser = "codex-items"

[dispatch]
exec_template = '''printf '%s\n' '{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"ok"}}' > <outputs_dir>/probe-events.jsonl'''
"#;

#[test]
fn harness_lint_probe_recovers_final_response_from_fake_transcript() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("probe-ok.toml");
    fs::write(&file, PROBE_OK_TOML).unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .args(["--probe", "--yes"])
        .assert()
        .success()
        .stdout(contains(
            "✓ live exec template: transcript final response recovered",
        ));
}

#[test]
fn harness_lint_probe_applies_descriptor_agent_environment() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("probe-env.toml");
    fs::write(
        &file,
        r#"
label = "probe-env"

[tools]
write = ["file_change"]
shell = ["command_execution"]

[transcript]
events_filename = "probe-events.jsonl"
parser = "codex-items"

[dispatch]
exec_template = '''[ "$PROBE_ENV" = "visible" ] && printf '%s\n' '{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"ok"}}' > <outputs_dir>/probe-events.jsonl'''

[dispatch.env]
PROBE_ENV = "visible"
"#,
    )
    .unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .args(["--probe", "--yes"])
        .assert()
        .success()
        .stdout(contains("✓ live exec template"));
}

#[test]
fn harness_lint_probe_runs_against_a_registered_name() {
    let tmp = TempDir::new().unwrap();
    write_project_descriptor(tmp.path(), "probe-ok.toml", PROBE_OK_TOML);

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint", "probe-ok", "--probe", "--yes"])
        .assert()
        .success()
        .stdout(contains("✓ live exec template"));
}

#[test]
fn harness_lint_probe_fails_when_transcript_is_missing() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("probe-bad.toml");
    fs::write(
        &file,
        PROBE_OK_TOML.replace(
            "exec_template = '''printf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"id\":\"m1\",\"type\":\"agent_message\",\"text\":\"ok\"}}' > <outputs_dir>/probe-events.jsonl'''",
            "exec_template = 'true'",
        ),
    )
    .unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .args(["--probe", "--yes"])
        .assert()
        .failure()
        .stderr(contains("✗").and(contains("probe-events.jsonl")));
}

#[test]
fn harness_lint_probe_aborts_without_yes_on_non_yes_stdin() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("probe-ok.toml");
    fs::write(&file, PROBE_OK_TOML).unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .args(["--probe"])
        .write_stdin("n\n")
        .assert()
        .failure()
        .stderr(
            contains("About to execute")
                .and(contains("aborted"))
                .and(contains("✓ live exec template").not()),
        );
}

#[test]
fn harness_lint_probe_timeout_kills_a_long_command() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("probe-slow.toml");
    fs::write(
        &file,
        "label = \"probe-slow\"\n\n[dispatch]\nexec_template = 'sleep 3'\n",
    )
    .unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .args(["--probe", "--yes", "--probe-timeout", "1"])
        .assert()
        .failure()
        .stderr(contains("✗").and(contains("timed out")));
}

#[test]
fn harness_lint_probe_without_exec_template_reports_nothing_to_run() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("no-dispatch.toml");
    fs::write(&file, "label = \"no-dispatch\"\n").unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .args(["--probe", "--yes"])
        .assert()
        .failure()
        .stderr(contains("✗").and(contains("dispatch.exec_template")));
}

#[test]
fn harness_lint_probe_does_not_run_after_static_checks_fail() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("broken.toml");
    fs::write(&file, "label = \"broken\"\nmystery = 1\n").unwrap();

    skill_eval()
        .current_dir(tmp.path())
        .args(["harness", "lint"])
        .arg(&file)
        .args(["--probe", "--yes"])
        .assert()
        .failure()
        .stderr(contains("mystery").and(contains("About to execute").not()));
}
