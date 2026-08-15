//! Host-tooling preflight for `run`: does this machine have what the generated
//! recipes ask an operator to paste?
//!
//! Unlike [`super::git`], a missing shell is a warning rather than an error.
//! `run` never dispatches — it prepares a workspace, and that workspace is
//! correct whatever shell prepared it. Preparing on Windows and dispatching from
//! WSL is a legitimate split, so this reports the gap and lets the run finish.

use std::path::Path;

use crate::core::{
    POSIX_RECIPE_TOOLS, POSIX_TOOLING_REQUIREMENT, posix_shell, require_posix_toolchain,
};

/// Warnings for a host that cannot run the recipes this run is about to
/// generate. Empty on a complete host.
pub(super) fn preflight_posix_tooling() -> Vec<String> {
    let shell = posix_shell();
    // Only probe for tools once a shell exists: `require_posix_toolchain`
    // resolves the same shell first, and reporting one failure beats reporting
    // the same absence twice in different words.
    let missing = match shell {
        Ok(_) => require_posix_toolchain(POSIX_RECIPE_TOOLS).err(),
        Err(_) => None,
    };
    tooling_warning(shell, missing.as_deref())
        .into_iter()
        .collect()
}

/// The operator warning for a host that cannot run the generated recipes, or
/// `None` when it can — a healthy run stays silent.
///
/// Two distinguishable failures. `shell` is `Err` when no `sh` exists at all,
/// and its message already carries [`POSIX_TOOLING_REQUIREMENT`], so the warning
/// only adds that the prepared workspace survives. `missing` is the tool absent
/// from a shell that *was* found — the Git for Windows case, which resolves a
/// shell but bundles no `jq` — and needs the requirement appended.
fn tooling_warning(shell: Result<&Path, &str>, missing: Option<&str>) -> Option<String> {
    if let Err(reason) = shell {
        return Some(format!(
            "{reason} The workspace and recipes below are still correct — prepare here and \
             dispatch them from a POSIX shell."
        ));
    }
    let reason = missing?;
    Some(format!(
        "{reason}. The parallel-dispatch and judge recipes in RUNBOOK.md are pipelines over that \
         toolchain and cannot run without it. {POSIX_TOOLING_REQUIREMENT}"
    ))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// A complete host stays silent — the preflight must not nag the common case.
    #[test]
    fn a_complete_host_produces_no_warning() {
        assert_eq!(tooling_warning(Ok(Path::new("/bin/sh")), None), None);
    }

    /// No shell at all: `posix_shell`'s own message already carries the declared
    /// requirement, so the warning adds only the reassurance that the prepared
    /// workspace is still correct.
    #[test]
    fn a_missing_shell_warns_with_the_declared_requirement() {
        let warning = tooling_warning(Err("no POSIX shell found. Use Git Bash or WSL."), None)
            .expect("a host with no POSIX shell must be told");
        assert!(warning.contains("no POSIX shell found"), "{warning}");
        assert!(warning.contains("Git Bash"), "{warning}");
    }

    /// A shell that is missing a recipe tool names the tool and the shell whose
    /// PATH was searched, and still points at the declared requirement — Git for
    /// Windows resolves a shell but bundles no `jq`, which is exactly this case.
    #[test]
    fn a_missing_recipe_tool_warns_naming_the_tool_and_the_requirement() {
        let warning = tooling_warning(
            Ok(Path::new("/opt/Git/usr/bin/sh.exe")),
            Some("jq is not on the PATH of /opt/Git/usr/bin/sh.exe"),
        )
        .expect("a shell without `jq` must be told");
        assert!(warning.contains("jq is not on the PATH"), "{warning}");
        assert!(warning.contains("/opt/Git/usr/bin/sh.exe"), "{warning}");
        assert!(warning.contains("RUNBOOK.md"), "{warning}");
        assert!(warning.contains("Git Bash"), "{warning}");
    }

    /// A resolved shell wins over a stale tool reason: the shell branch is only
    /// reachable when discovery itself failed.
    #[test]
    fn the_shell_failure_takes_precedence() {
        let warning = tooling_warning(Err("no POSIX shell found."), Some("jq is missing"))
            .expect("a missing shell warns");
        assert!(!warning.contains("jq is missing"), "{warning}");
    }
}
