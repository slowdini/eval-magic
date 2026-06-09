//! Unit tests for the staged-skill lifecycle, split by concern (file-length
//! guideline). Shared fixtures live here; each submodule covers one area:
//! [`stage`] (single-skill staging + registration), [`sibling`] (sibling
//! staging), [`cleanup`] (teardown + restore).

use super::*;
use std::fs;
use std::path::Path;

mod cleanup;
mod sibling;
mod stage;

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn read_manifest(skills_dir: &Path) -> SiblingManifest {
    serde_json::from_str(&read(&skills_dir.join(STAGED_SIBLING_MANIFEST))).unwrap()
}

/// `<root>/src-skills` with alpha (+evals +helper), beta, gamma.
fn build_source_skills(root: &Path) -> std::path::PathBuf {
    let src = root.join("src-skills");
    write(&src.join("alpha").join("SKILL.md"), "alpha content");
    write(&src.join("alpha").join("helper.md"), "alpha helper");
    write(&src.join("alpha").join("evals").join("evals.json"), "{}");
    write(&src.join("beta").join("SKILL.md"), "beta content");
    write(&src.join("gamma").join("SKILL.md"), "gamma content");
    src
}
