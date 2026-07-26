use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::core::{GitOutput, run_git};

#[derive(Debug, Deserialize)]
struct IsolationDispatch {
    #[serde(default)]
    tasks: Vec<IsolationTask>,
}

#[derive(Debug, Deserialize)]
struct IsolationTask {
    eval_id: String,
    condition: String,
    #[serde(default)]
    run_index: Option<u32>,
    eval_root: Option<String>,
}

/// Probe task roots recorded by older or hand-authored dispatch manifests.
/// Canonical task repositories and genuine non-repository roots are clean;
/// ancestor discovery or an unavailable/failed probe weakens validity.
pub(super) fn collect_warnings(iteration_dir: &Path, warnings: &mut Vec<String>) {
    let Ok(raw) = fs::read_to_string(iteration_dir.join("dispatch.json")) else {
        return;
    };
    let Ok(dispatch) = serde_json::from_str::<IsolationDispatch>(&raw) else {
        return;
    };
    for task in dispatch.tasks {
        let Some(eval_root) = task.eval_root.as_deref() else {
            continue;
        };
        let output = run_git(&["rev-parse", "--show-toplevel"], Path::new(eval_root));
        if let Some(warning) = isolation_warning(&task, Path::new(eval_root), &output) {
            warnings.push(warning);
        }
    }
}

fn isolation_warning(task: &IsolationTask, eval_root: &Path, output: &GitOutput) -> Option<String> {
    let run = task
        .run_index
        .map(|index| format!("/run-{index}"))
        .unwrap_or_default();
    let label = format!("{}/{}{run}", task.eval_id, task.condition);
    if output.status == Some(0) {
        let Ok(stdout) = std::str::from_utf8(&output.stdout) else {
            return Some(format!(
                "{label} could not verify task Git isolation because git returned a non-UTF-8 \
                 top-level path — this legacy task may include unrelated Git state."
            ));
        };
        let reported = Path::new(stdout.trim());
        let expected = fs::canonicalize(eval_root).ok();
        let actual = fs::canonicalize(reported).ok();
        return match (expected, actual) {
            (Some(expected), Some(actual)) if expected == actual => None,
            (Some(expected), Some(actual)) => Some(format!(
                "{label} resolved Git top-level {} instead of task root {} (ancestor repository) \
                 — this legacy task may include unrelated Git state.",
                actual.display(),
                expected.display()
            )),
            _ => Some(format!(
                "{label} could not verify task Git isolation because the reported top-level could \
                 not be canonicalized — this legacy task may include unrelated Git state."
            )),
        };
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.is_some() && stderr.to_ascii_lowercase().contains("not a git repository") {
        return None;
    }
    let detail = stderr.lines().next().unwrap_or("unknown Git error").trim();
    Some(format!(
        "{label} could not verify task Git isolation ({detail}) — this legacy task may include \
         unrelated Git state."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_local_and_non_repository_roots_but_warns_when_unavailable() {
        let temp = tempfile::TempDir::new().unwrap();
        let task = IsolationTask {
            eval_id: "e1".into(),
            condition: "with_skill".into(),
            run_index: None,
            eval_root: Some(temp.path().to_string_lossy().into_owned()),
        };
        let local = GitOutput {
            status: Some(0),
            stdout: format!("{}\n", temp.path().display()).into_bytes(),
            stderr: Vec::new(),
        };
        assert!(isolation_warning(&task, temp.path(), &local).is_none());

        let no_repository = GitOutput {
            status: Some(128),
            stdout: Vec::new(),
            stderr: b"fatal: not a git repository (or any parent)\n".to_vec(),
        };
        assert!(isolation_warning(&task, temp.path(), &no_repository).is_none());

        let unavailable = GitOutput {
            status: None,
            stdout: Vec::new(),
            stderr: b"No such file or directory".to_vec(),
        };
        let warning = isolation_warning(&task, temp.path(), &unavailable).unwrap();
        assert!(warning.contains("e1/with_skill"), "{warning}");
        assert!(warning.contains("could not verify"), "{warning}");
    }
}
