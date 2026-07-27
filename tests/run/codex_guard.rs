//! Codex guard arguments across eval-agent and judge dispatch.

use crate::helpers::*;
use std::fs;
use std::path::Path;

#[test]
fn codex_guarded_eval_keeps_hook_bypass_while_judges_omit_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "codex"])
        .assert()
        .success();

    let runbook = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    let (eval_section, judge_and_after) = runbook
        .split_once("## 2. Dispatch the judge agents, then finalize")
        .expect("runbook has separate eval and judge sections");
    assert!(
        eval_section.contains("--dangerously-bypass-hook-trust"),
        "guarded eval agents must trust their staged hook: {eval_section}"
    );
    let judge_section = judge_and_after
        .split_once("## 3. Read the result")
        .map(|(section, _)| section)
        .unwrap_or(judge_and_after);
    assert!(
        !judge_section.contains("--dangerously-bypass-hook-trust"),
        "judges run outside guarded task envs: {judge_section}"
    );

    let dispatch = read_json(&iteration_dir(&cwd).join("dispatch.json"));
    for task in dispatch["tasks"].as_array().unwrap() {
        let outputs = Path::new(task["outputs_dir"].as_str().unwrap());
        fs::create_dir_all(outputs).unwrap();
        fs::write(outputs.join("final-message.md"), "Reviewed.\n").unwrap();
    }

    let ingest = skill_eval()
        .current_dir(&cwd)
        .args(["ingest", "--skill-dir"])
        .arg(&skill_dir)
        .args([
            "--skill",
            "mr-review",
            "--harness",
            "codex",
            "--iteration",
            "1",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(ingest.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("Dispatch each judge task"), "{stdout}");
    assert!(
        stdout.contains("codex --ask-for-approval never exec"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("--dangerously-bypass-hook-trust"),
        "ingest judge recipe must not inherit task guard flags: {stdout}"
    );
}
