//! Paired evidence reports for assertion-free exploration.

use crate::helpers::skill_eval;
use predicates::str::contains;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    skill_dir: PathBuf,
    iteration_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let skill_dir = root.join("skills");
        let skill = skill_dir.join("demo");
        fs::create_dir_all(skill.join("evals")).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: test\n---\n\nbody\n",
        )
        .unwrap();
        fs::write(
            skill.join("evals/evals.json"),
            serde_json::to_string_pretty(&json!({
                "skill_name": "demo",
                "evals": [{
                    "id": "implement-feature",
                    "prompt": "Implement the feature.",
                    "expected_output": "The feature works."
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let iteration_dir = root.join(".eval-magic/demo/iteration-1");
        fs::create_dir_all(&iteration_dir).unwrap();
        fs::write(
            iteration_dir.join("conditions.json"),
            serde_json::to_string_pretty(&json!({
                "mode": "new-skill",
                "conditions": [
                    {"name": "with_skill", "skill_path": "/copied/demo/SKILL.md"},
                    {"name": "without_skill", "skill_path": null}
                ],
                "timestamp": "2026-08-23T12:00:00.000Z"
            }))
            .unwrap(),
        )
        .unwrap();

        Self {
            _tmp: tmp,
            root,
            skill_dir,
            iteration_dir,
        }
    }

    fn write_evidence(&self, condition: &str, body: &str) {
        let dir = self
            .iteration_dir
            .join("eval-implement-feature")
            .join(condition);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("judge-evidence.md"), body).unwrap();
    }

    fn write_run_evidence(&self, condition: &str, run: u32, body: &str) {
        let dir = self
            .iteration_dir
            .join("eval-implement-feature")
            .join(condition)
            .join(format!("run-{run}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("judge-evidence.md"), body).unwrap();
    }

    fn command(&self) -> assert_cmd::Command {
        self.command_for_eval("implement-feature")
    }

    fn command_for_eval(&self, eval_id: &str) -> assert_cmd::Command {
        let mut command = skill_eval();
        command
            .current_dir(&self.root)
            .args(["compare", "--skill-dir"])
            .arg(&self.skill_dir)
            .args(["--skill", "demo", "--iteration", "1", "--eval", eval_id]);
        command
    }

    fn report_path(&self) -> PathBuf {
        self.iteration_dir
            .join("compare")
            .join("implement-feature.md")
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn compare_writes_both_arms_without_authored_assertions() {
    let fixture = Fixture::new();
    fixture.write_evidence(
        "with_skill",
        "# Judge evidence bundle\n\nwith prompt, final message, transcript, and diff\n",
    );
    fixture.write_evidence(
        "without_skill",
        "# Judge evidence bundle\n\nwithout prompt, final message, transcript, and diff\n",
    );

    let report_path = fixture.report_path();
    fs::create_dir_all(report_path.parent().unwrap()).unwrap();
    fs::write(&report_path, "stale report\n").unwrap();
    fixture
        .command()
        .assert()
        .success()
        .stdout(contains(format!("Wrote {}", report_path.display())));

    let report = read(&report_path);
    assert!(report.contains("`with_skill`"), "{report}");
    assert!(report.contains("`without_skill`"), "{report}");
    assert!(report.contains("with prompt, final message, transcript, and diff"));
    assert!(report.contains("without prompt, final message, transcript, and diff"));
    assert!(report.contains("exploratory"), "{report}");
    assert!(report.contains("not a grade"), "{report}");
    assert!(!report.contains("stale report"), "{report}");
}

#[test]
fn compare_pairs_multi_run_evidence_numerically_and_names_validity_artifacts() {
    let fixture = Fixture::new();
    for run in [10, 2] {
        fixture.write_run_evidence(
            "with_skill",
            run,
            &format!("# Judge evidence bundle\n\nwith run {run}\n"),
        );
        fixture.write_run_evidence(
            "without_skill",
            run,
            &format!("# Judge evidence bundle\n\nwithout run {run}\n"),
        );
    }
    fs::write(fixture.iteration_dir.join("plugin-shadow.json"), "{}\n").unwrap();
    fs::write(fixture.iteration_dir.join("guard-denials.json"), "{}\n").unwrap();

    fixture.command().assert().success();

    let report = read(&fixture.report_path());
    let run_2 = report.find("## Run 2").unwrap();
    let run_10 = report.find("## Run 10").unwrap();
    assert!(run_2 < run_10, "runs are numerically ordered: {report}");
    for evidence in [
        "with run 2",
        "without run 2",
        "with run 10",
        "without run 10",
    ] {
        assert!(report.contains(evidence), "missing {evidence}: {report}");
    }
    assert!(report.contains("plugin-shadow.json"), "{report}");
    assert!(report.contains("guard-denials.json"), "{report}");
}

#[test]
fn compare_names_the_missing_run_and_writes_no_partial_report() {
    let fixture = Fixture::new();
    for run in [1, 2] {
        fixture.write_run_evidence(
            "with_skill",
            run,
            &format!("# Judge evidence bundle\n\nwith run {run}\n"),
        );
    }
    fixture.write_run_evidence(
        "without_skill",
        1,
        "# Judge evidence bundle\n\nwithout run 1\n",
    );

    fixture
        .command()
        .assert()
        .failure()
        .stderr(contains("missing run-2 from condition 'without_skill'"));

    assert!(!fixture.report_path().exists());
}

#[test]
fn compare_preserves_revision_condition_names_and_embedded_provenance() {
    let fixture = Fixture::new();
    fs::write(
        fixture.iteration_dir.join("conditions.json"),
        serde_json::to_string_pretty(&json!({
            "mode": "revision",
            "baseline": "baseline",
            "conditions": [
                {"name": "old_skill", "skill_path": "/copied/old/SKILL.md"},
                {"name": "new_skill", "skill_path": "/copied/new/SKILL.md"}
            ],
            "timestamp": "2026-08-23T12:00:00.000Z"
        }))
        .unwrap(),
    )
    .unwrap();
    fixture.write_evidence(
        "old_skill",
        "# Judge evidence bundle\n\nSkill revision: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
    );
    fixture.write_evidence(
        "new_skill",
        "# Judge evidence bundle\n\nSkill revision: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n",
    );

    fixture.command().assert().success();

    let report = read(&fixture.report_path());
    assert!(report.contains("Mode: `revision`"), "{report}");
    assert!(report.contains("`old_skill`"), "{report}");
    assert!(report.contains("`new_skill`"), "{report}");
    assert!(report.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    assert!(report.contains("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
}

#[test]
fn compare_unknown_eval_lists_iteration_eval_ids() {
    let fixture = Fixture::new();
    fixture.write_evidence("with_skill", "# Judge evidence bundle\n\nwith evidence\n");
    fixture.write_evidence(
        "without_skill",
        "# Judge evidence bundle\n\nwithout evidence\n",
    );

    fixture
        .command_for_eval("missing-eval")
        .assert()
        .failure()
        .stderr(contains("eval 'missing-eval' is not present"))
        .stderr(contains("available evals: implement-feature"));
}

#[test]
fn compare_missing_arm_preserves_a_prior_report() {
    let fixture = Fixture::new();
    fixture.write_evidence("with_skill", "# Judge evidence bundle\n\nwith evidence\n");
    let report_path = fixture.report_path();
    fs::create_dir_all(report_path.parent().unwrap()).unwrap();
    fs::write(&report_path, "prior complete report\n").unwrap();

    fixture
        .command()
        .assert()
        .failure()
        .stderr(contains("condition 'without_skill' is missing"));

    assert_eq!(read(&report_path), "prior complete report\n");
}

#[test]
fn compare_rejects_empty_evidence_before_replacing_the_report() {
    let fixture = Fixture::new();
    fixture.write_evidence("with_skill", " \n");
    fixture.write_evidence(
        "without_skill",
        "# Judge evidence bundle\n\nwithout evidence\n",
    );
    let report_path = fixture.report_path();
    fs::create_dir_all(report_path.parent().unwrap()).unwrap();
    fs::write(&report_path, "prior complete report\n").unwrap();

    fixture
        .command()
        .assert()
        .failure()
        .stderr(contains("empty evidence for implement-feature/with_skill"))
        .stderr(contains("eval-magic ingest"));

    assert_eq!(read(&report_path), "prior complete report\n");
}

#[test]
fn compare_fences_untrusted_markdown_with_a_safe_delimiter() {
    let fixture = Fixture::new();
    let evidence = "# Judge evidence bundle\n\n````\nuntrusted fence\n````\n";
    fixture.write_evidence("with_skill", evidence);
    fixture.write_evidence("without_skill", evidence);

    fixture.command().assert().success();

    let report = read(&fixture.report_path());
    assert!(
        report.contains("`````markdown\n# Judge evidence bundle"),
        "the wrapper must be longer than the evidence fence: {report}"
    );
    assert!(
        report.contains("````\nuntrusted fence\n````"),
        "the evidence remains intact: {report}"
    );
}
