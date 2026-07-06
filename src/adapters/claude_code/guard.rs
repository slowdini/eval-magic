//! Claude Code write-guard hook: install + verdict shape.
//!
//! Arms the guard by merging a `PreToolUse` hook into the env's
//! `.claude/settings.local.json`; each `claude -p` dispatch runs from the env
//! dir, so it loads and enforces the hook. The hook invokes the hidden `guard`
//! subcommand (a stable on-disk contract), whose deny verdict uses Claude
//! Code's native `hookSpecificOutput` shape.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::sandbox::decide::{GuardMarker, decide};
use crate::sandbox::install::{
    GUARD_MANIFEST, GUARD_MARKER, write_json, write_manifest, write_marker,
};
use crate::sandbox::{now_ms, parse_tool_call};

/// Tool names the Claude Code PreToolUse hook fires on.
const HOOK_MATCHER: &str = "Write|Edit|MultiEdit|NotebookEdit|Bash";

/// Arm the write guard using Claude Code's project-local hook surface. Returns
/// the staged marker path.
pub(crate) fn install_guard(
    stage_root: &Path,
    guard_exe: &Path,
    ttl: Option<Duration>,
) -> io::Result<PathBuf> {
    let skills_dir = stage_root.join(".claude").join("skills");
    fs::create_dir_all(&skills_dir)?;

    let marker_path = skills_dir.join(GUARD_MARKER);
    write_marker(&marker_path, stage_root, ttl)?;

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

    write_manifest(
        &skills_dir.join(GUARD_MANIFEST),
        &settings_path,
        settings_existed,
        backup,
        &marker_path,
    )?;

    Ok(marker_path)
}

/// Evaluate a PreToolUse hook `payload` (the JSON Claude Code sends on stdin)
/// against `marker`. Returns the serialized deny verdict to print on stdout when
/// the call is blocked — Claude Code's native `hookSpecificOutput` shape — or
/// `None` to allow (print nothing). An empty or malformed payload is treated as
/// allow.
pub(crate) fn guard_decision(payload: &str, marker: Option<GuardMarker>) -> Option<String> {
    let (tool_name, tool_input) = parse_tool_call(payload)?;

    let decision = decide(&tool_name, &tool_input, marker.as_ref(), now_ms());
    if decision.allow {
        return None;
    }
    Some(
        serde_json::to_string(&json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": decision.reason,
            }
        }))
        .expect("deny verdict serializes"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::install::{iso_millis, teardown_guard};
    use crate::sandbox::{guard_is_armed, now_ms};
    use chrono::DateTime;
    use tempfile::TempDir;

    struct Case {
        _tmp: TempDir,
        stage_root: PathBuf,
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

    fn skills_dir(stage_root: &Path) -> PathBuf {
        stage_root.join(".claude").join("skills")
    }

    fn settings_path(stage_root: &Path) -> PathBuf {
        stage_root.join(".claude").join("settings.local.json")
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn absolutize(p: &Path) -> PathBuf {
        std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
    }

    /// A live marker (active, no expiry → unexpired) scoped to one root.
    fn marker() -> GuardMarker {
        GuardMarker {
            active: Some(true),
            allowed_roots: Some(vec!["/work/.eval-magic".to_string()]),
            expires_at: None,
        }
    }

    #[test]
    fn install_writes_an_active_marker_hook_and_manifest() {
        let c = setup();
        let exe = Path::new("/g/eval-magic");
        install_guard(&c.stage_root, exe, None).unwrap();

        let marker = read_json(&skills_dir(&c.stage_root).join(GUARD_MARKER));
        assert_eq!(marker["active"], json!(true));
        let expires = marker["expiresAt"].as_str().unwrap();
        let exp_ms = DateTime::parse_from_rfc3339(expires)
            .unwrap()
            .timestamp_millis();
        assert!(exp_ms > now_ms());
        let env = absolutize(&c.stage_root).display().to_string();
        assert!(
            marker["allowedRoots"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.as_str().unwrap() == env)
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
    fn marker_scopes_allowed_roots_to_the_env_and_temp_only() {
        let c = setup();
        let exe = Path::new("/g/eval-magic");
        install_guard(&c.stage_root, exe, None).unwrap();

        let marker = read_json(&skills_dir(&c.stage_root).join(GUARD_MARKER));
        let roots: Vec<String> = marker["allowedRoots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_str().unwrap().to_string())
            .collect();

        // The guard boundary is the isolated env (stage_root) plus temp — nothing
        // above it. The parent workspace tree must NOT be an allowed root, or the
        // agent could write into sibling iterations / the meta dir above `env/`.
        let env = absolutize(&c.stage_root).display().to_string();
        let temp = absolutize(&std::env::temp_dir()).display().to_string();
        assert_eq!(roots, vec![env, temp]);
        assert!(
            !roots.iter().any(|r| r.ends_with(".eval-magic")),
            "workspace_root must not be an allowed root: {roots:?}"
        );
    }

    #[test]
    fn hook_command_invokes_the_binary_guard_subcommand() {
        let c = setup();
        let exe = Path::new("/g/eval-magic");
        let marker = install_guard(&c.stage_root, exe, None).unwrap();
        let settings = read_json(&settings_path(&c.stage_root));
        let command = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            command,
            format!("\"/g/eval-magic\" guard \"{}\"", marker.display())
        );
    }

    #[test]
    fn teardown_deletes_settings_it_created() {
        let c = setup();
        let exe = Path::new("/g/eval-magic");
        install_guard(&c.stage_root, exe, None).unwrap();
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

        let exe = Path::new("/g/eval-magic");
        install_guard(&c.stage_root, exe, None).unwrap();
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
    fn guard_is_armed_ignores_missing_inactive_expired_and_malformed_markers() {
        let c = setup();
        let marker_path = skills_dir(&c.stage_root).join(GUARD_MARKER);
        fs::create_dir_all(skills_dir(&c.stage_root)).unwrap();

        assert!(!guard_is_armed(&c.stage_root));

        fs::write(
            &marker_path,
            serde_json::to_string(&json!({ "active": false })).unwrap(),
        )
        .unwrap();
        assert!(!guard_is_armed(&c.stage_root));

        fs::write(
            &marker_path,
            serde_json::to_string(&json!({
                "active": true,
                "expiresAt": iso_millis(now_ms() - 60_000),
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(!guard_is_armed(&c.stage_root));

        fs::write(&marker_path, "not json").unwrap();
        assert!(!guard_is_armed(&c.stage_root));
    }

    #[test]
    fn teardown_sweeps_a_stray_marker_even_without_a_manifest() {
        let c = setup();
        fs::create_dir_all(skills_dir(&c.stage_root)).unwrap();
        fs::write(skills_dir(&c.stage_root).join(GUARD_MARKER), "{}").unwrap();
        assert!(teardown_guard(&c.stage_root));
        assert!(!skills_dir(&c.stage_root).join(GUARD_MARKER).exists());
    }

    #[test]
    fn allows_returns_none() {
        let payload = r#"{ "tool_name": "Read", "tool_input": { "file_path": "/etc/passwd" } }"#;
        assert_eq!(guard_decision(payload, Some(marker())), None);
    }

    #[test]
    fn deny_returns_pretooluse_deny_json() {
        let payload = r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#;
        let out = guard_decision(payload, Some(marker())).expect("should deny");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(
            v["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("outside")
        );
    }

    #[test]
    fn no_marker_allows_everything() {
        let payload = r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#;
        assert_eq!(guard_decision(payload, None), None);
    }

    #[test]
    fn empty_or_malformed_payload_fails_open() {
        assert_eq!(guard_decision("", Some(marker())), None);
        assert_eq!(guard_decision("not json", Some(marker())), None);
    }
}
