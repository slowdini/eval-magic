//! Command-check cache identity and legacy-result detection.

use std::fs;
use std::path::Path;

use crate::core::AssertionCommandCheck;
use crate::pipeline::error::PipelineError;

/// Digest of a check's authored definition, so reuse never crosses an edit.
pub(super) fn definition_digest(check: &AssertionCommandCheck) -> String {
    crate::core::fs::fnv1a_hex(
        serde_json::to_string(check)
            .expect("an authored command_check serializes")
            .as_bytes(),
    )
}

/// Digest the exact runner-owned record bytes that a result grades.
pub(super) fn run_record_digest(path: &Path) -> Result<String, PipelineError> {
    Ok(crate::core::fs::fnv1a_hex(&fs::read(path)?))
}

/// True when an incomplete task already carries a result left by an older
/// grader. Presence alone is enough for the safety warning: a malformed legacy
/// result still means held-out setup or its command may have touched the env.
pub(super) fn has_cached_results(results_dir: &Path) -> Result<bool, PipelineError> {
    if !results_dir.exists() {
        return Ok(false);
    }
    for entry in fs::read_dir(results_dir)? {
        if entry?
            .path()
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            return Ok(true);
        }
    }
    Ok(false)
}
