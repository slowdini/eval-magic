//! Which `evals.json` supplies the assertions a finished run is graded against.
//!
//! The copy an iteration froze under `.skills/<skill>/` defines the treatment:
//! what the agent loaded must not change after the dispatch it explains. The
//! assertions in that same file are a different thing — the measuring
//! instrument — and the documented workflow authors them *after* the run they
//! grade, from the evidence the run produced. So grading reads only the fields
//! it consumes from the live `evals.json`, leaves everything else frozen, and
//! records which file it read them from.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::core::fs::fnv1a_hex;
use crate::core::{AssertionSource, EvalsConfig};
use crate::pipeline::error::PipelineError;
use crate::validation::validate_evals_config;

/// The resolved eval config grading measures with, plus where its assertions
/// came from.
#[derive(Debug)]
pub struct GradingInstrument {
    /// The frozen config with each eval's grading fields refreshed from the
    /// live file, matched by eval id.
    pub evals: EvalsConfig,
    pub source: AssertionSource,
    /// Returned rather than printed: library modules never write to the
    /// terminal, the CLI handler owns how a warning reads.
    pub warnings: Vec<String>,
    frozen_root: PathBuf,
    live_root: Option<PathBuf>,
    /// Eval ids whose grading fields differ from what the run froze.
    refreshed: BTreeSet<String>,
}

impl GradingInstrument {
    /// An instrument with no live tree to consult: everything is graded by
    /// what the iteration captured.
    pub fn frozen(evals: EvalsConfig, root: &Path) -> Self {
        Self {
            source: frozen_source(&evals, &evals_path(root)),
            evals,
            warnings: Vec::new(),
            frozen_root: root.to_path_buf(),
            live_root: None,
            refreshed: BTreeSet::new(),
        }
    }

    /// Eval ids whose grading fields differ from the run-time copy.
    pub fn refreshed_eval_ids(&self) -> impl Iterator<Item = &str> {
        self.refreshed.iter().map(String::as_str)
    }

    /// The skill directory this eval's held-out `command_check.setup_files`
    /// resolve against: the live tree for an eval whose assertions came from
    /// there, the frozen copy for one still graded by what the run captured.
    pub fn setup_root_for(&self, eval_id: &str) -> &Path {
        match &self.live_root {
            Some(live) if self.refreshed.contains(eval_id) => live,
            _ => &self.frozen_root,
        }
    }
}

/// `<skill_dir>/evals/evals.json`.
fn evals_path(skill_dir: &Path) -> PathBuf {
    skill_dir.join("evals").join("evals.json")
}

fn read_config(path: &Path) -> Result<EvalsConfig, PipelineError> {
    parse_config(&std::fs::read_to_string(path)?, path)
}

fn parse_config(raw: &str, path: &Path) -> Result<EvalsConfig, PipelineError> {
    // Name the file in the parse error too. Validation failures already carry
    // their source, and a bare "key must be a string at line 1" would send an
    // operator hunting through two identically-named files.
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|error| PipelineError::Message(format!("{}: {error}", path.display())))?;
    Ok(validate_evals_config(&value, &path.to_string_lossy())?)
}

/// Digest over exactly the fields grading reads, in config order, so it
/// identifies the instrument without depending on how the file is formatted.
fn instrument_digest(config: &EvalsConfig) -> String {
    let fields = config
        .evals
        .iter()
        .map(|eval| {
            json!({
                "id": eval.id,
                "assertions": eval.assertions,
                "skill_should_trigger": eval.skill_should_trigger,
            })
        })
        .collect::<Vec<_>>();
    fnv1a_hex(
        serde_json::to_string(&fields)
            .expect("eval grading fields serialize")
            .as_bytes(),
    )
}

/// Resolve the assertion set to grade with: the frozen config, with every
/// eval's grading fields taken from the live file where the two disagree.
pub fn resolve_grading_instrument(
    frozen_skill_dir: &Path,
    live_skill_dir: &Path,
) -> Result<GradingInstrument, PipelineError> {
    let frozen_path = evals_path(frozen_skill_dir);
    let live_path = evals_path(live_skill_dir);
    // An iteration prepared before skills were sourced has no copy to compare
    // against, so the one file on disk is both the record of what ran and the
    // instrument. Reading it twice would only invent a comparison.
    if frozen_path == live_path {
        return Ok(GradingInstrument::frozen(
            read_config(&frozen_path)?,
            frozen_skill_dir,
        ));
    }

    let mut evals = read_config(&frozen_path)?;
    let mut warnings = Vec::new();
    // A missing live tree is ordinary (graded from another machine, or the
    // skill moved) and leaves the iteration's own copy as the instrument. An
    // *invalid* one is not: the operator just edited the file grading measures
    // with, and grading around a broken edit is the failure being fixed here.
    let live = match std::fs::read_to_string(&live_path) {
        Ok(raw) => Some(parse_config(&raw, &live_path).map_err(|error| {
            PipelineError::Message(format!(
                "{error}\nThis file supplies the assertions grading measures with. Fix it (`eval-magic validate` reports the same errors) and re-run grade."
            ))
        })?),
        Err(error) => {
            warnings.push(format!(
                "could not read {} ({error}); grading with the assertions frozen at run time in {}.",
                live_path.display(),
                frozen_path.display()
            ));
            None
        }
    };
    let Some(live) = live else {
        return Ok(GradingInstrument {
            source: frozen_source(&evals, &frozen_path),
            evals,
            warnings,
            frozen_root: frozen_skill_dir.to_path_buf(),
            live_root: None,
            refreshed: BTreeSet::new(),
        });
    };

    let mut refreshed = BTreeSet::new();
    for eval in &mut evals.evals {
        let Some(authored) = live.evals.iter().find(|other| other.id == eval.id) else {
            continue;
        };
        if authored.assertions == eval.assertions
            && authored.skill_should_trigger == eval.skill_should_trigger
        {
            continue;
        }
        eval.assertions = authored.assertions.clone();
        eval.skill_should_trigger = authored.skill_should_trigger;
        refreshed.insert(eval.id.clone());
    }

    // An eval set that has moved on since the run is not an error — the ids
    // that still match are graded normally — but each side of the mismatch
    // changes what the operator should expect to see graded.
    for authored in &live.evals {
        if !evals.evals.iter().any(|eval| eval.id == authored.id) {
            warnings.push(format!(
                "eval '{}' is defined in {} but not in this iteration, so it was never dispatched here. Build a new iteration to grade it.",
                authored.id,
                live_path.display()
            ));
        }
    }
    for eval in &evals.evals {
        if !live.evals.iter().any(|authored| authored.id == eval.id) {
            warnings.push(format!(
                "eval '{}' is no longer defined in {}; grading it with the assertions frozen at run time.",
                eval.id,
                live_path.display()
            ));
        }
    }

    let source = if refreshed.is_empty() {
        frozen_source(&evals, &frozen_path)
    } else {
        AssertionSource {
            path: live_path.to_string_lossy().into_owned(),
            digest: instrument_digest(&evals),
            refreshed: true,
        }
    };
    Ok(GradingInstrument {
        evals,
        source,
        warnings,
        frozen_root: frozen_skill_dir.to_path_buf(),
        live_root: Some(live_skill_dir.to_path_buf()),
        refreshed,
    })
}

/// The source record for a grading measured by what the run captured.
fn frozen_source(evals: &EvalsConfig, frozen_path: &Path) -> AssertionSource {
    AssertionSource {
        path: frozen_path.to_string_lossy().into_owned(),
        digest: instrument_digest(evals),
        refreshed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_grading_instrument;
    use serde_json::{Value, json};
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Write `<root>/evals/evals.json` and return the skill root.
    fn skill(root: &Path, config: &Value) -> PathBuf {
        fs::create_dir_all(root.join("evals")).unwrap();
        fs::write(
            root.join("evals").join("evals.json"),
            serde_json::to_string_pretty(config).unwrap(),
        )
        .unwrap();
        root.to_path_buf()
    }

    fn config(assertions: Value) -> Value {
        json!({
            "skill_name": "mr-review",
            "codebase": { "path": "." },
            "evals": [{
                "id": "implement-feature",
                "prompt": "Implement the feature.",
                "expected_output": "A working feature.",
                "assertions": assertions,
            }],
        })
    }

    /// The bug in #295: assertions authored after the run must be what grading
    /// measures, while the frozen copy keeps defining what ran.
    #[test]
    fn live_assertions_replace_the_frozen_copys_per_eval_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frozen = skill(&tmp.path().join("frozen"), &config(json!([])));
        let live = skill(
            &tmp.path().join("live"),
            &config(json!([
                { "id": "quality", "type": "llm_judge", "rubric": "Is it well tested?" }
            ])),
        );

        let instrument = resolve_grading_instrument(&frozen, &live).unwrap();

        let assertions = instrument.evals.evals[0].assertions.as_deref().unwrap();
        assert_eq!(assertions.len(), 1);
        assert!(instrument.source.refreshed);
        assert_eq!(
            instrument.source.path,
            live.join("evals").join("evals.json").to_string_lossy()
        );
        assert_eq!(
            instrument.refreshed_eval_ids().collect::<Vec<_>>(),
            vec!["implement-feature"]
        );
    }

    /// The frozen copy stays authoritative for everything that defined what
    /// ran; only the fields grading reads come from the live file.
    #[test]
    fn merging_keeps_everything_the_run_was_defined_by() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut frozen_config = config(json!([]));
        frozen_config["evals"][0]["prompt"] = json!("The prompt the agent actually got.");
        frozen_config["evals"][0]["files"] = json!(["fixture.txt"]);
        let frozen = skill(&tmp.path().join("frozen"), &frozen_config);

        let mut live_config = config(json!([
            { "id": "quality", "type": "llm_judge", "rubric": "Is it well tested?" }
        ]));
        live_config["evals"][0]["prompt"] = json!("A prompt edited after the run.");
        live_config["evals"][0]["files"] = json!(["different.txt"]);
        live_config["evals"][0]["skill_should_trigger"] = json!(false);
        let live = skill(&tmp.path().join("live"), &live_config);

        let instrument = resolve_grading_instrument(&frozen, &live).unwrap();

        let eval = &instrument.evals.evals[0];
        assert_eq!(eval.prompt, "The prompt the agent actually got.");
        assert_eq!(eval.files.as_deref().unwrap(), ["fixture.txt"]);
        // skill_should_trigger gates the skill-invocation meta-check and is
        // read only at grade time, so it refreshes with the assertions.
        assert_eq!(eval.skill_should_trigger, Some(false));
    }

    /// A `command_check` added after the run names a held-out setup file that
    /// exists only in the live tree, so setup files follow the assertions that
    /// reference them.
    #[test]
    fn setup_files_resolve_from_the_tree_that_supplied_the_assertions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut two = config(json!([]));
        two["evals"].as_array_mut().unwrap().push(json!({
            "id": "fix-bug",
            "prompt": "Fix the bug.",
            "expected_output": "A fix.",
        }));
        let frozen = skill(&tmp.path().join("frozen"), &two);

        let mut live_config = two.clone();
        live_config["evals"][0]["assertions"] = json!([
            { "id": "build-passes", "type": "command_check", "command": "true" }
        ]);
        let live = skill(&tmp.path().join("live"), &live_config);

        let instrument = resolve_grading_instrument(&frozen, &live).unwrap();

        assert_eq!(instrument.setup_root_for("implement-feature"), live);
        // Untouched evals keep resolving against what the run froze.
        assert_eq!(instrument.setup_root_for("fix-bug"), frozen);
    }

    /// Nothing edited: grading reports the copy the run froze, so a zero-task
    /// summary is never ambiguous about which file produced it.
    #[test]
    fn an_unedited_instrument_reports_the_run_time_copy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let same = config(json!([
            { "id": "quality", "type": "llm_judge", "rubric": "Is it well tested?" }
        ]));
        let frozen = skill(&tmp.path().join("frozen"), &same);
        let live = skill(&tmp.path().join("live"), &same);

        let instrument = resolve_grading_instrument(&frozen, &live).unwrap();

        assert!(!instrument.source.refreshed);
        assert_eq!(
            instrument.source.path,
            frozen.join("evals").join("evals.json").to_string_lossy()
        );
        assert_eq!(instrument.refreshed_eval_ids().count(), 0);
        assert!(instrument.warnings.is_empty());
    }

    /// The live tree can be gone — graded from another machine, or the skill
    /// moved. Grading still runs on what the iteration captured, and says so.
    #[test]
    fn an_unreadable_live_file_falls_back_to_the_frozen_copy_with_a_warning() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frozen = skill(
            &tmp.path().join("frozen"),
            &config(json!([
                { "id": "quality", "type": "llm_judge", "rubric": "Is it well tested?" }
            ])),
        );
        let live = tmp.path().join("moved-away");

        let instrument = resolve_grading_instrument(&frozen, &live).unwrap();

        assert!(!instrument.source.refreshed);
        assert_eq!(
            instrument.source.path,
            frozen.join("evals").join("evals.json").to_string_lossy()
        );
        assert_eq!(
            instrument.evals.evals[0]
                .assertions
                .as_deref()
                .unwrap()
                .len(),
            1
        );
        let warning = instrument.warnings.join("\n");
        assert!(
            warning.contains(&live.join("evals").join("evals.json").display().to_string()),
            "the warning names the file it could not read: {warning}"
        );
        assert!(
            warning.contains("frozen"),
            "the warning says what grading fell back to: {warning}"
        );
    }

    /// A whole eval authored after the run has no dispatched cells to grade.
    /// Silently emitting nothing for it is the same defect one level up.
    #[test]
    fn an_eval_only_in_the_live_file_warns_that_it_was_never_dispatched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frozen = skill(&tmp.path().join("frozen"), &config(json!([])));

        let mut live_config = config(json!([]));
        live_config["evals"].as_array_mut().unwrap().push(json!({
            "id": "handle-conflict",
            "prompt": "Resolve the conflict.",
            "expected_output": "A resolution.",
            "assertions": [
                { "id": "quality", "type": "llm_judge", "rubric": "Is it resolved?" }
            ],
        }));
        let live = skill(&tmp.path().join("live"), &live_config);

        let instrument = resolve_grading_instrument(&frozen, &live).unwrap();

        let warning = instrument.warnings.join("\n");
        assert!(warning.contains("handle-conflict"), "{warning}");
        assert!(warning.contains("never dispatched"), "{warning}");
        assert_eq!(instrument.evals.evals.len(), 1);
    }

    /// An eval deleted from the live file was still measured by this run, so it
    /// keeps the assertions the run captured rather than silently losing them.
    #[test]
    fn an_eval_dropped_from_the_live_file_keeps_its_run_time_assertions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frozen = skill(
            &tmp.path().join("frozen"),
            &config(json!([
                { "id": "quality", "type": "llm_judge", "rubric": "Is it well tested?" }
            ])),
        );
        let mut live_config = config(json!([]));
        live_config["evals"][0]["id"] = json!("renamed-eval");
        let live = skill(&tmp.path().join("live"), &live_config);

        let instrument = resolve_grading_instrument(&frozen, &live).unwrap();

        assert_eq!(
            instrument.evals.evals[0]
                .assertions
                .as_deref()
                .unwrap()
                .len(),
            1
        );
        let warning = instrument.warnings.join("\n");
        assert!(warning.contains("implement-feature"), "{warning}");
        assert!(warning.contains("no longer"), "{warning}");
    }

    /// The operator just edited the file grading measures with. Falling back to
    /// the frozen assertions here would grade around a broken edit, which is
    /// the exact silence this module exists to remove.
    #[test]
    fn an_invalid_live_file_fails_grading_instead_of_grading_around_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frozen = skill(&tmp.path().join("frozen"), &config(json!([])));
        let live = tmp.path().join("live");
        fs::create_dir_all(live.join("evals")).unwrap();
        fs::write(live.join("evals").join("evals.json"), "{ not json").unwrap();

        let error = resolve_grading_instrument(&frozen, &live)
            .unwrap_err()
            .to_string();

        assert!(
            error.contains(&live.join("evals").join("evals.json").display().to_string()),
            "the failure names the live file: {error}"
        );
        assert!(
            error.contains("assertions"),
            "the failure says why that file matters to grading: {error}"
        );
        assert!(
            error.contains("eval-magic validate"),
            "the failure names the way out: {error}"
        );
    }

    /// The digest identifies the instrument, not the file's formatting: two
    /// gradings can be compared without diffing the evals.json they read.
    #[test]
    fn the_digest_tracks_the_graded_assertion_set_not_the_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let assertions = json!([
            { "id": "quality", "type": "llm_judge", "rubric": "Is it well tested?" }
        ]);
        let frozen = skill(&tmp.path().join("frozen"), &config(assertions.clone()));

        // Same assertions, a prompt edited after the run, and different bytes.
        let mut reformatted = config(assertions);
        reformatted["evals"][0]["prompt"] = json!("An edit that grading does not read.");
        let unchanged = skill(&tmp.path().join("unchanged"), &reformatted);

        let edited_config = config(json!([
            { "id": "quality", "type": "llm_judge", "rubric": "Does it rank by severity?" }
        ]));
        let edited = skill(&tmp.path().join("edited"), &edited_config);

        let baseline = resolve_grading_instrument(&frozen, &unchanged).unwrap();
        let same = resolve_grading_instrument(&frozen, &frozen).unwrap();
        let different = resolve_grading_instrument(&frozen, &edited).unwrap();

        assert_eq!(baseline.source.digest, same.source.digest);
        assert_ne!(baseline.source.digest, different.source.digest);
    }

    /// An iteration prepared before skills were sourced has no copy, so the
    /// live file is the only instrument there has ever been — not a refresh.
    #[test]
    fn without_a_frozen_copy_the_one_file_on_disk_is_the_instrument() {
        let tmp = tempfile::TempDir::new().unwrap();
        let only = skill(
            &tmp.path().join("skill"),
            &config(json!([
                { "id": "quality", "type": "llm_judge", "rubric": "Is it well tested?" }
            ])),
        );

        let instrument = resolve_grading_instrument(&only, &only).unwrap();

        assert!(!instrument.source.refreshed);
        assert_eq!(
            instrument.source.path,
            only.join("evals").join("evals.json").to_string_lossy()
        );
        assert_eq!(instrument.setup_root_for("implement-feature"), only);
        assert!(instrument.warnings.is_empty());
        assert_eq!(
            instrument.evals.evals[0]
                .assertions
                .as_deref()
                .unwrap()
                .len(),
            1
        );
    }
}
