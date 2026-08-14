//! Shared write-guard arm/disarm machinery.
//!
//! The descriptor-driven engine in `crate::adapters::guard` installs each
//! harness's native hook surface using the marker/manifest helpers here. The
//! marker lists the allowed roots; arming also initializes a privacy-safe raw
//! denial log under the guarded task env, and disarming preserves that evidence.
//! The original hook or plugin surface is backed up verbatim in a manifest so
//! [`teardown_guard`] restores it exactly.
//!
//! The hook command points at the running binary (`std::env::current_exe`), so
//! there is no separate hook script to ship and no interpreter to select.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::fs::write_json;
use chrono::{DateTime, SecondsFormat};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::Harness;

use super::now_ms;
use super::{guard::read_marker, marker_is_armed};

/// Marker file (under the staged skills dir) that arms the guard.
pub const GUARD_MARKER: &str = ".slow-powers-eval-guard.json";
/// Manifest recording what install changed, so teardown can restore it.
pub const GUARD_MANIFEST: &str = ".slow-powers-eval-guard-manifest.json";
/// Per-task raw denial log directory, relative to the guarded eval root.
pub(crate) const GUARD_DENIALS_DIR: &str = ".eval-magic-outputs";
/// Per-task raw denial log filename.
pub(crate) const GUARD_DENIALS_LOG: &str = "guard-denials.jsonl";

/// Default lifetime of an armed guard. Bounds how long a crashed run's hook can
/// linger before it is treated as expired (see `super::decide`).
const GUARD_TTL: Duration = Duration::from_secs(6 * 60 * 60); // 6h

/// Restoration record written beside the marker. The field names are the
/// on-disk manifest format — keep them stable so older manifests stay readable.
#[derive(Debug, Serialize, Deserialize)]
struct GuardManifest {
    created_at: String,
    settings_path: String,
    settings_existed: bool,
    settings_backup: Option<String>,
    marker_path: String,
}

/// Format epoch milliseconds as `2026-06-08T12:00:00.000Z` — RFC 3339 with
/// millisecond precision, the timestamp format every artifact uses.
pub(crate) fn iso_millis(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Lexically absolutize a path (no disk access, no symlink resolution) for the
/// allowed-roots list.
fn absolutize(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The guard's sole allowed write root: the isolated env (`stage_root`, the
/// agent-under-test's cwd). The staged skills dir and the per-task outputs dir
/// both live *inside* `stage_root`, so this root covers every legitimate agent
/// write. Scoping to the env — not the parent `.eval-magic/` or the host temp
/// directory — keeps the guard boundary identical to the isolation boundary:
/// the agent can't reach a sibling iteration or the `iteration-N/` meta tree
/// above its cwd. eval-magic's own above-env writes (e.g. `benchmark.json`) are
/// not gated here: they run as non-mutating `eval-magic` subprocesses the
/// guard's Bash classifier passes.
fn marker_allowed_roots(stage_root: &Path) -> Vec<String> {
    vec![absolutize(stage_root).display().to_string()]
}

/// Write the guard marker that arms the hook for `stage_root`. The guard is a
/// no-op until this marker exists and is unexpired, so the hook is inert
/// outside an active run. `ttl` overrides the default 6h lifetime.
pub(crate) fn write_marker(
    marker_path: &Path,
    stage_root: &Path,
    ttl: Option<Duration>,
) -> io::Result<()> {
    let expires_ms = now_ms() + ttl.unwrap_or(GUARD_TTL).as_millis() as i64;
    let denial_log_path = absolutize(&stage_root.join(GUARD_DENIALS_DIR).join(GUARD_DENIALS_LOG));
    if let Some(parent) = denial_log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Re-arming starts a fresh observation window for this task. Teardown
    // intentionally leaves this evidence file in place.
    fs::write(&denial_log_path, "")?;
    write_json(
        marker_path,
        &json!({
            "active": true,
            "allowedRoots": marker_allowed_roots(stage_root),
            "expiresAt": iso_millis(expires_ms),
            "denialLogPath": denial_log_path,
        }),
    )
}

/// Write the restoration manifest recording what install changed.
pub(crate) fn write_manifest(
    manifest_path: &Path,
    settings_path: &Path,
    settings_existed: bool,
    settings_backup: Option<String>,
    marker_path: &Path,
) -> io::Result<()> {
    let manifest = GuardManifest {
        created_at: iso_millis(now_ms()),
        settings_path: settings_path.display().to_string(),
        settings_existed,
        settings_backup,
        marker_path: marker_path.display().to_string(),
    };
    write_json(manifest_path, &manifest)
}

/// Disarm the guard: restore the original harness hook file (or delete it if we
/// created it) and remove the marker + manifest, for every harness's skills
/// dir. Safe to call when no guard is installed. Returns true if a guard was
/// found and torn down.
pub fn teardown_guard(stage_root: &Path) -> bool {
    let mut torn = false;
    for harness in Harness::known() {
        let adapter = crate::adapters::adapter_for(harness);
        // A harness without a skills dir cannot host a guard (descriptor
        // validation rejects the combination), so there is nothing to sweep.
        if let Some(skills_dir) = adapter.skills_dir(stage_root) {
            torn |= teardown_guard_from_skills_dir(&skills_dir);
        }
        if let Some(hook_dir) = adapter.guard_hook_cleanup_dir(stage_root) {
            let _ = prune_if_empty(&hook_dir);
        }
    }
    torn
}

/// True when any harness has a live guard marker under `stage_root`.
pub(crate) fn guard_is_armed(stage_root: &Path) -> bool {
    let now = now_ms();
    Harness::known().any(|harness| {
        crate::adapters::adapter_for(harness)
            .skills_dir(stage_root)
            .is_some_and(|skills_dir| {
                let marker_path = skills_dir.join(GUARD_MARKER);
                marker_is_armed(read_marker(&marker_path).as_ref(), now)
            })
    })
}

fn teardown_guard_from_skills_dir(skills_dir: &Path) -> bool {
    let manifest_path = skills_dir.join(GUARD_MANIFEST);
    let marker_path = skills_dir.join(GUARD_MARKER);

    let Ok(manifest_text) = fs::read_to_string(&manifest_path) else {
        // No manifest — still sweep a stray marker so the guard can't stay armed.
        if marker_path.exists() {
            let _ = fs::remove_file(&marker_path);
            return true;
        }
        return false;
    };

    let Ok(manifest) = serde_json::from_str::<GuardManifest>(&manifest_text) else {
        // Corrupt manifest: drop both files and report a teardown.
        let _ = fs::remove_file(&manifest_path);
        let _ = fs::remove_file(&marker_path);
        return true;
    };

    match (manifest.settings_existed, &manifest.settings_backup) {
        (true, Some(backup)) => {
            let _ = fs::write(&manifest.settings_path, backup);
        }
        _ => {
            let _ = fs::remove_file(&manifest.settings_path);
        }
    }
    let _ = fs::remove_file(&manifest.marker_path);
    let _ = fs::remove_file(&manifest_path);
    true
}

fn prune_if_empty(dir: &Path) -> io::Result<()> {
    if dir.exists() && fs::read_dir(dir)?.next().is_none() {
        fs::remove_dir_all(dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Case {
        _tmp: TempDir,
        stage_root: PathBuf,
    }

    /// The marker path as it appears *inside* a JSON string value: the hook
    /// command embeds it, so every Windows separator is escaped to `\\`.
    /// Interpolating `display()` raw builds an expectation that is not even
    /// valid JSON, and the byte pin then fails on a difference the file does
    /// not have.
    fn json_string_body(path: &Path) -> String {
        let quoted = serde_json::to_string(&path.to_string_lossy()).unwrap();
        quoted[1..quoted.len() - 1].to_string()
    }

    fn setup() -> Case {
        let tmp = TempDir::new().unwrap();
        let stage_root = tmp.path().join("stage");
        fs::create_dir_all(&stage_root).unwrap();
        Case {
            _tmp: tmp,
            stage_root,
        }
    }

    #[test]
    fn teardown_is_a_safe_no_op_when_nothing_is_installed() {
        let c = setup();
        assert!(!teardown_guard(&c.stage_root));
    }

    #[test]
    fn teardown_sweeps_stray_marker_without_manifest() {
        let c = setup();
        // A stray marker (no manifest) in any harness's skills dir must still
        // be swept so the guard can't stay armed.
        for skills_dir in [
            c.stage_root.join(".claude").join("skills"),
            c.stage_root.join(".opencode").join("skills"),
        ] {
            fs::create_dir_all(&skills_dir).unwrap();
            let marker = skills_dir.join(GUARD_MARKER);
            fs::write(&marker, "{}").unwrap();
            assert!(teardown_guard(&c.stage_root));
            assert!(!marker.exists(), "stray marker at {marker:?} was not swept");
        }
    }

    /// Byte-pin of a fresh Claude hook file: armed envs and their backups are an
    /// on-disk compatibility surface, so the merged settings must keep this exact
    /// 2-space-pretty shape, key order, and trailing newline.
    #[test]
    fn claude_install_writes_this_exact_hook_file() {
        let c = setup();
        let adapter = crate::adapters::adapter_for(Harness::resolve("claude-code").unwrap());
        let marker = adapter
            .install_guard(&c.stage_root, Path::new("/g/eval-magic"), None)
            .unwrap();

        let settings =
            fs::read_to_string(c.stage_root.join(".claude").join("settings.local.json")).unwrap();
        let expected = format!(
            r#"{{
  "hooks": {{
    "PreToolUse": [
      {{
        "matcher": "Write|Edit|MultiEdit|NotebookEdit|Bash",
        "hooks": [
          {{
            "type": "command",
            "command": "\"/g/eval-magic\" guard \"{marker}\""
          }}
        ]
      }}
    ]
  }}
}}
"#,
            marker = json_string_body(&marker)
        );
        assert_eq!(settings, expected);
    }

    /// Byte-pin of a fresh Codex hook file — same compatibility contract as the
    /// Claude pin, plus the Codex-only `timeout`/`statusMessage` keys.
    #[test]
    fn codex_install_writes_this_exact_hook_file() {
        let c = setup();
        let adapter = crate::adapters::adapter_for(Harness::resolve("codex").unwrap());
        let marker = adapter
            .install_guard(&c.stage_root, Path::new("/g/eval-magic"), None)
            .unwrap();

        let hooks = fs::read_to_string(c.stage_root.join(".codex").join("hooks.json")).unwrap();
        let expected = format!(
            r#"{{
  "hooks": {{
    "PreToolUse": [
      {{
        "matcher": "^Bash$|^apply_patch$|^Edit$|^Write$",
        "hooks": [
          {{
            "type": "command",
            "command": "\"/g/eval-magic\" guard-codex \"{marker}\"",
            "timeout": 30,
            "statusMessage": "Checking eval write boundary"
          }}
        ]
      }}
    ]
  }}
}}
"#,
            marker = json_string_body(&marker)
        );
        assert_eq!(hooks, expected);
    }

    #[test]
    fn guard_is_armed_detects_claude_or_codex_marker() {
        let c = setup();
        let exe = Path::new("/g/eval-magic");
        let claude = crate::adapters::adapter_for(Harness::resolve("claude-code").unwrap());
        claude.install_guard(&c.stage_root, exe, None).unwrap();
        assert!(guard_is_armed(&c.stage_root));
        teardown_guard(&c.stage_root);
        assert!(!guard_is_armed(&c.stage_root));

        let codex = crate::adapters::adapter_for(Harness::resolve("codex").unwrap());
        codex.install_guard(&c.stage_root, exe, None).unwrap();
        assert!(guard_is_armed(&c.stage_root));
    }
}
