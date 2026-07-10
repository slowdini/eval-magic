//! Codex write-guard hook: install + verdict shape.
//!
//! Arms the guard by merging a `PreToolUse` hook into the env's
//! `.codex/hooks.json`; dispatches must pass `--dangerously-bypass-hook-trust`
//! so the vetted project-local hook actually runs. The hook invokes the hidden
//! `guard-codex` subcommand (a stable on-disk contract), whose block verdict
//! uses Codex's native `{ "decision": "block", "reason": "..." }` shape.

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

/// Tool names the Codex PreToolUse hook fires on.
pub(crate) const HOOK_MATCHER: &str = "^Bash$|^apply_patch$|^Edit$|^Write$";

/// Arm the write guard using Codex's project-local hook surface. Returns the
/// staged marker path.
pub(crate) fn install_guard(
    stage_root: &Path,
    guard_exe: &Path,
    ttl: Option<Duration>,
) -> io::Result<PathBuf> {
    let skills_dir = stage_root.join(".agents").join("skills");
    fs::create_dir_all(&skills_dir)?;

    let marker_path = skills_dir.join(GUARD_MARKER);
    write_marker(&marker_path, stage_root, ttl)?;

    let hooks_path = stage_root.join(".codex").join("hooks.json");
    if let Some(parent) = hooks_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let hooks_existed = hooks_path.exists();
    let backup = if hooks_existed {
        Some(fs::read_to_string(&hooks_path)?)
    } else {
        None
    };

    let mut hooks: Value = backup
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| json!({}));
    let hooks_obj = hooks
        .as_object_mut()
        .expect("hooks.json root is a JSON object")
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let pre = hooks_obj
        .as_object_mut()
        .expect("hooks is a JSON object")
        .entry("PreToolUse")
        .or_insert_with(|| json!([]));
    let command = format!(
        "\"{}\" guard-codex \"{}\"",
        guard_exe.display(),
        marker_path.display()
    );
    pre.as_array_mut()
        .expect("PreToolUse is an array")
        .push(json!({
            "matcher": HOOK_MATCHER,
            "hooks": [
                {
                    "type": "command",
                    "command": command,
                    "timeout": 30,
                    "statusMessage": "Checking eval write boundary",
                }
            ],
        }));
    write_json(&hooks_path, &hooks)?;

    write_manifest(
        &skills_dir.join(GUARD_MANIFEST),
        &hooks_path,
        hooks_existed,
        backup,
        &marker_path,
    )?;

    Ok(marker_path)
}

/// The hook-config dir the Codex guard writes under `stage_root`; teardown
/// prunes it when the restored config leaves it empty.
pub(crate) fn hook_cleanup_dir(stage_root: &Path) -> PathBuf {
    stage_root.join(".codex")
}

/// Evaluate a PreToolUse hook `payload` (the JSON Codex sends on stdin) against
/// `marker`. Codex's hook contract blocks by returning `{ "decision": "block",
/// "reason": "..." }` on stdout — kept separate from Claude Code's
/// `hookSpecificOutput` shape so both harnesses use their native conventions.
pub(crate) fn guard_decision(payload: &str, marker: Option<GuardMarker>) -> Option<String> {
    let (tool_name, tool_input) = parse_tool_call(payload)?;
    let decision = decide(&tool_name, &tool_input, marker.as_ref(), now_ms());
    if decision.allow {
        return None;
    }
    Some(
        serde_json::to_string(&json!({
            "decision": "block",
            "reason": decision.reason,
        }))
        .expect("Codex block verdict serializes"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::install::teardown_guard;
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

    fn codex_hooks_path(stage_root: &Path) -> PathBuf {
        stage_root.join(".codex").join("hooks.json")
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
    fn codex_install_writes_project_hook_marker_and_manifest() {
        let c = setup();
        let exe = Path::new("/g/eval-magic");
        install_guard(&c.stage_root, exe, None).unwrap();

        let marker = read_json(
            &c.stage_root
                .join(".agents")
                .join("skills")
                .join(GUARD_MARKER),
        );
        assert_eq!(marker["active"], json!(true));
        // The Codex guard shares the env-scoped roots: the staged `.agents/skills`
        // dir lives inside `stage_root`, so the single env root already covers it.
        let env = absolutize(&c.stage_root).display().to_string();
        assert!(
            marker["allowedRoots"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.as_str().unwrap() == env)
        );

        let hooks = read_json(&codex_hooks_path(&c.stage_root));
        let hook = &hooks["hooks"]["PreToolUse"][0];
        assert!(hook["matcher"].as_str().unwrap().contains("apply_patch"));
        assert!(
            hook["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("guard-codex")
        );
        assert!(
            c.stage_root
                .join(".agents")
                .join("skills")
                .join(GUARD_MANIFEST)
                .exists()
        );
    }

    #[test]
    fn codex_teardown_restores_pre_existing_hooks_json_verbatim() {
        let c = setup();
        fs::create_dir_all(c.stage_root.join(".codex")).unwrap();
        let original = format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "hooks": {
                    "PostToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [{ "type": "command", "command": "echo ok" }]
                        }
                    ]
                }
            }))
            .unwrap()
        );
        fs::write(codex_hooks_path(&c.stage_root), &original).unwrap();

        install_guard(&c.stage_root, Path::new("/g/eval-magic"), None).unwrap();
        assert!(
            fs::read_to_string(codex_hooks_path(&c.stage_root))
                .unwrap()
                .contains("guard-codex")
        );

        teardown_guard(&c.stage_root);
        assert_eq!(
            fs::read_to_string(codex_hooks_path(&c.stage_root)).unwrap(),
            original
        );
    }

    #[test]
    fn codex_deny_returns_decision_block_json() {
        let payload = r#"{ "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": { "command": "npm install left-pad" } }"#;
        let out = guard_decision(payload, Some(marker())).expect("should block");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("blocked Bash"));
    }

    #[test]
    fn codex_apply_patch_outside_allowed_roots_blocks() {
        let payload = r#"{ "hook_event_name": "PreToolUse", "tool_name": "apply_patch", "tool_input": { "files": ["/etc/passwd"] } }"#;
        let out = guard_decision(payload, Some(marker())).expect("should block");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("apply_patch"));
    }

    #[test]
    fn codex_apply_patch_inside_allowed_roots_allows() {
        let payload = r#"{ "hook_event_name": "PreToolUse", "tool_name": "apply_patch", "tool_input": { "files": ["/work/.eval-magic/out.md"] } }"#;
        assert_eq!(guard_decision(payload, Some(marker())), None);
    }
}
