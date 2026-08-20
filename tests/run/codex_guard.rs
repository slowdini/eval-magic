//! Codex guard arguments across eval-agent and judge dispatch.

use crate::helpers::*;

/// A guarded eval agent runs with the hook-trust bypass so it trusts the guard
/// staged in its own env; a judge, which runs from the iteration directory
/// outside every guarded env, must not inherit it. The runner applies the
/// distinction when it spawns, so what is checkable from the artifacts is the
/// command the manifest says it will spawn. The judge half is pinned in
/// `tests/run/judges.rs`.
#[test]
fn a_guarded_eval_dispatch_keeps_the_hook_bypass() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (skill_dir, cwd) = setup(tmp.path(), DEFAULT_EVALS);
    skill_eval()
        .current_dir(&cwd)
        .args(["run", "--skill-dir"])
        .arg(&skill_dir)
        .args(["--skill", "mr-review", "--harness", "codex"])
        .assert()
        .success();

    let manifest = read_str(&iteration_dir(&cwd).join("dispatch-manifest.md"));
    assert!(
        manifest.contains("--dangerously-bypass-hook-trust"),
        "guarded eval agents must trust their staged hook: {manifest}"
    );

    // The runbook is harness-agnostic now — it names runner commands, so no
    // harness flag of either kind appears in it.
    let runbook = read_str(&iteration_dir(&cwd).join("RUNBOOK.md"));
    assert!(
        !runbook.contains("--dangerously-bypass-hook-trust"),
        "{runbook}"
    );
}
