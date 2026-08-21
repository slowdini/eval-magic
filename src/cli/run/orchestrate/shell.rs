//! Host-shell preflight for `run`: can this machine dispatch what it is about
//! to prepare?
//!
//! Unlike [`super::git`], a missing shell is a warning rather than an error.
//! `run` never dispatches — it prepares a workspace, and that workspace is
//! correct whatever shell prepared it, so this reports the gap and lets the run
//! finish. `dispatch` is where the absence becomes fatal.
//!
//! The gap is host-local: `dispatch` spawns each harness command line with the
//! workspace's own absolute paths, so the shell it resolves has to resolve
//! those. Windows users keep preparation and dispatch inside the same WSL
//! environment. See [`POSIX_TOOLING_REQUIREMENT`] for the declared rule the
//! warning defers to.

use std::path::Path;

use crate::core::posix_shell;

/// Warnings for a host that cannot dispatch the run it is about to prepare.
/// Empty on a complete host.
pub(super) fn preflight_posix_tooling() -> Vec<String> {
    tooling_warning(posix_shell()).into_iter().collect()
}

/// The operator warning for a host with no POSIX shell, or `None` when one
/// resolves — a healthy run stays silent.
///
/// `posix_shell`'s own message already carries [`POSIX_TOOLING_REQUIREMENT`],
/// so the warning only adds that the prepared workspace survives the gap.
fn tooling_warning(shell: Result<&Path, &str>) -> Option<String> {
    let reason = shell.err()?;
    Some(format!(
        "{reason} The workspace below is still correct — dispatch it from a POSIX shell on \
         this host."
    ))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// A complete host stays silent — the preflight must not nag the common case.
    #[test]
    fn a_complete_host_produces_no_warning() {
        assert_eq!(tooling_warning(Ok(Path::new("/bin/sh"))), None);
    }

    /// No shell at all: `posix_shell`'s own message already carries the declared
    /// requirement, so the warning adds only the reassurance that the prepared
    /// workspace is still correct.
    #[test]
    fn a_missing_shell_warns_with_the_declared_requirement() {
        let warning = tooling_warning(Err(
            "no POSIX shell found. On Windows, run eval-magic inside WSL; native Windows is unsupported.",
        ))
            .expect("a host with no POSIX shell must be told");
        assert!(warning.contains("no POSIX shell found"), "{warning}");
        assert!(warning.contains("WSL"), "{warning}");
        assert!(
            warning.contains("native Windows is unsupported"),
            "{warning}"
        );
    }

    /// An unqualified "dispatch it from a POSIX shell" could invite a caller to
    /// cross filesystem namespaces. Confining it to the preparing host keeps the
    /// generated absolute paths valid.
    #[test]
    fn a_missing_shell_confines_dispatch_to_the_host_that_prepared_the_workspace() {
        let warning = tooling_warning(Err("no POSIX shell found."))
            .expect("a host with no POSIX shell must be told");
        assert!(
            warning.contains("this host"),
            "the warning must keep dispatch on the preparing host: {warning}"
        );
    }
}
