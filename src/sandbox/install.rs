//! Arm / disarm the write guard.
//!
//! [`install_guard`] writes a marker listing the allowed roots and merges a
//! `PreToolUse` hook into `.claude/settings.local.json` that runs this binary's
//! hidden `guard` subcommand on every Write/Edit/Bash. The original settings file
//! is backed up verbatim in a manifest so [`teardown_guard`] restores it exactly.
//!
//! The hook command points at the running binary (`std::env::current_exe`), so
//! there is no separate hook script to ship and no interpreter to select.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::now_ms;

/// Marker file (under `<stageRoot>/.claude/skills/`) that arms the guard.
pub const GUARD_MARKER: &str = ".slow-powers-eval-guard.json";
/// Manifest recording what install changed, so teardown can restore it.
pub const GUARD_MANIFEST: &str = ".slow-powers-eval-guard-manifest.json";

/// Default lifetime of an armed guard. Bounds how long a crashed run's hook can
/// linger before it is treated as expired (see `super::decide`).
const GUARD_TTL: Duration = Duration::from_secs(6 * 60 * 60); // 6h

/// Tool names the PreToolUse hook fires on.
const HOOK_MATCHER: &str = "Write|Edit|MultiEdit|NotebookEdit|Bash";

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
fn iso_millis(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Lexically absolutize a path (no disk access, no symlink resolution) for the
/// allowed-roots list.
fn absolutize(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Write `value` as 2-space-pretty JSON with a trailing newline — the stable
/// on-disk format for every artifact this binary writes.
fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    text.push('\n');
    fs::write(path, text)
}

/// Arm the write guard for an eval run. Returns the marker path. The guard is a
/// no-op until this marker exists and is unexpired, so the hook is inert outside
/// an active run. `guard_exe` is the path the hook invokes (normally
/// `std::env::current_exe()`); `ttl` overrides the default 6h lifetime.
pub fn install_guard(
    stage_root: &Path,
    workspace_root: &Path,
    guard_exe: &Path,
    ttl: Option<Duration>,
) -> io::Result<PathBuf> {
    let skills_dir = stage_root.join(".claude").join("skills");
    fs::create_dir_all(&skills_dir)?;

    let marker_path = skills_dir.join(GUARD_MARKER);
    let allowed_roots = vec![
        absolutize(workspace_root).display().to_string(),
        absolutize(&skills_dir).display().to_string(),
        absolutize(&std::env::temp_dir()).display().to_string(),
    ];
    let expires_ms = now_ms() + ttl.unwrap_or(GUARD_TTL).as_millis() as i64;
    write_json(
        &marker_path,
        &json!({
            "active": true,
            "allowedRoots": allowed_roots,
            "expiresAt": iso_millis(expires_ms),
        }),
    )?;

    let settings_path = stage_root.join(".claude").join("settings.local.json");
    let settings_existed = settings_path.exists();
    let backup = if settings_existed {
        Some(fs::read_to_string(&settings_path)?)
    } else {
        None
    };

    // Start from the existing settings (or an empty object), preserving key
    // order, then append the PreToolUse hook entry.
    let mut settings: Value = backup
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| json!({}));
    let hooks = settings
        .as_object_mut()
        .expect("settings is a JSON object")
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let pre = hooks
        .as_object_mut()
        .expect("hooks is a JSON object")
        .entry("PreToolUse")
        .or_insert_with(|| json!([]));
    let command = format!(
        "\"{}\" guard \"{}\"",
        guard_exe.display(),
        marker_path.display()
    );
    pre.as_array_mut()
        .expect("PreToolUse is an array")
        .push(json!({
            "matcher": HOOK_MATCHER,
            "hooks": [ { "type": "command", "command": command } ],
        }));
    write_json(&settings_path, &settings)?;

    let manifest = GuardManifest {
        created_at: iso_millis(now_ms()),
        settings_path: settings_path.display().to_string(),
        settings_existed,
        settings_backup: backup,
        marker_path: marker_path.display().to_string(),
    };
    write_json(
        &skills_dir.join(GUARD_MANIFEST),
        &serde_json::to_value(&manifest)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
    )?;

    Ok(marker_path)
}

/// Disarm the guard: restore the original `settings.local.json` (or delete it if
/// we created it) and remove the marker + manifest. Safe to call when no guard is
/// installed. Returns true if a guard was found and torn down.
pub fn teardown_guard(stage_root: &Path) -> bool {
    let skills_dir = stage_root.join(".claude").join("skills");
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Case {
        _tmp: TempDir,
        stage_root: PathBuf,
        workspace_root: PathBuf,
    }

    fn setup() -> Case {
        let tmp = TempDir::new().unwrap();
        let stage_root = tmp.path().join("stage");
        fs::create_dir_all(&stage_root).unwrap();
        let workspace_root = stage_root.join("skills-workspace");
        Case {
            _tmp: tmp,
            stage_root,
            workspace_root,
        }
    }

    fn skills_dir(stage_root: &Path) -> PathBuf {
        stage_root.join(".claude").join("skills")
    }

    fn settings_path(stage_root: &Path) -> PathBuf {
        stage_root.join(".claude").join("settings.local.json")
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn install_writes_an_active_marker_hook_and_manifest() {
        let c = setup();
        let exe = Path::new("/g/skill-eval");
        install_guard(&c.stage_root, &c.workspace_root, exe, None).unwrap();

        let marker = read_json(&skills_dir(&c.stage_root).join(GUARD_MARKER));
        assert_eq!(marker["active"], json!(true));
        let expires = marker["expiresAt"].as_str().unwrap();
        let exp_ms = DateTime::parse_from_rfc3339(expires)
            .unwrap()
            .timestamp_millis();
        assert!(exp_ms > now_ms());
        assert!(
            marker["allowedRoots"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.as_str().unwrap().contains("skills-workspace"))
        );

        let settings = read_json(&settings_path(&c.stage_root));
        let hook = &settings["hooks"]["PreToolUse"][0];
        assert!(hook["matcher"].as_str().unwrap().contains("Write"));
        assert!(
            hook["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("guard")
        );

        assert!(skills_dir(&c.stage_root).join(GUARD_MANIFEST).exists());
    }

    #[test]
    fn hook_command_invokes_the_binary_guard_subcommand() {
        let c = setup();
        let exe = Path::new("/g/skill-eval");
        let marker = install_guard(&c.stage_root, &c.workspace_root, exe, None).unwrap();
        let settings = read_json(&settings_path(&c.stage_root));
        let command = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            command,
            format!("\"/g/skill-eval\" guard \"{}\"", marker.display())
        );
    }

    #[test]
    fn teardown_deletes_settings_it_created() {
        let c = setup();
        let exe = Path::new("/g/skill-eval");
        install_guard(&c.stage_root, &c.workspace_root, exe, None).unwrap();
        assert!(settings_path(&c.stage_root).exists());

        assert!(teardown_guard(&c.stage_root));
        assert!(!settings_path(&c.stage_root).exists());
        assert!(!skills_dir(&c.stage_root).join(GUARD_MARKER).exists());
        assert!(!skills_dir(&c.stage_root).join(GUARD_MANIFEST).exists());
    }

    #[test]
    fn teardown_restores_a_pre_existing_settings_verbatim() {
        let c = setup();
        fs::create_dir_all(c.stage_root.join(".claude")).unwrap();
        let original = format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "permissions": { "allow": ["Bash(ls)"] }
            }))
            .unwrap()
        );
        fs::write(settings_path(&c.stage_root), &original).unwrap();

        let exe = Path::new("/g/skill-eval");
        install_guard(&c.stage_root, &c.workspace_root, exe, None).unwrap();
        // hook present while armed
        assert!(
            fs::read_to_string(settings_path(&c.stage_root))
                .unwrap()
                .contains("PreToolUse")
        );

        teardown_guard(&c.stage_root);
        assert_eq!(
            fs::read_to_string(settings_path(&c.stage_root)).unwrap(),
            original
        );
    }

    #[test]
    fn teardown_is_a_safe_no_op_when_nothing_is_installed() {
        let c = setup();
        assert!(!teardown_guard(&c.stage_root));
    }

    #[test]
    fn teardown_sweeps_a_stray_marker_even_without_a_manifest() {
        let c = setup();
        fs::create_dir_all(skills_dir(&c.stage_root)).unwrap();
        fs::write(skills_dir(&c.stage_root).join(GUARD_MARKER), "{}").unwrap();
        assert!(teardown_guard(&c.stage_root));
        assert!(!skills_dir(&c.stage_root).join(GUARD_MARKER).exists());
    }
}
