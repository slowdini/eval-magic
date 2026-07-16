//! The generic write-guard engine: one install + verdict implementation
//! rendered from a descriptor's `[guard]` data block.
//!
//! Arming merges one PreToolUse hook entry (the descriptor's `hook_entry` JSON
//! template) into the harness's hook-config file (`hooks_file`) and stages the
//! marker/manifest via [`crate::sandbox::install`]. The verdict side feeds a
//! hook payload through the shared arbiter ([`crate::sandbox::decide`]) and
//! serializes the descriptor's `verdict_template` on deny. Template key order
//! is authored in the descriptor and serialized verbatim (`serde_json` keeps
//! insertion order), so verdict bytes and hook-file shape are pinned by data,
//! not code.
//!
//! Guard blocks exist only in embedded built-in descriptors — the guard fails
//! open, so [`super::descriptor::layers::check_user_layer_restrictions`] bars
//! user layers from declaring one.

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

use super::descriptor::{GuardSection, subst};

/// Arm the write guard: marker + manifest under `skills_dir` (absolute,
/// resolved by the caller), one hook entry merged into the descriptor's
/// `hooks_file`. Returns the staged marker path.
///
/// Template parse failures panic (`expect`): descriptor validation proved the
/// templates at load time, and arming runs in the orchestrator where a loud
/// failure beats a silently unarmed guard.
pub(crate) fn install_guard(
    guard: &GuardSection,
    skills_dir: &Path,
    stage_root: &Path,
    guard_exe: &Path,
    ttl: Option<Duration>,
) -> io::Result<PathBuf> {
    fs::create_dir_all(skills_dir)?;

    let marker_path = skills_dir.join(GUARD_MARKER);
    write_marker(&marker_path, stage_root, ttl)?;

    let hooks_path = resolve_rel(stage_root, &guard.hooks_file);
    if let Some(parent) = hooks_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let hooks_existed = hooks_path.exists();
    let backup = if hooks_existed {
        Some(fs::read_to_string(&hooks_path)?)
    } else {
        None
    };

    // Start from the existing hook config (or an empty object), preserving key
    // order, then append the rendered hook entry.
    let mut config: Value = backup
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| json!({}));
    let exe = guard_exe.display().to_string();
    let marker = marker_path.display().to_string();
    let command = subst(
        &guard.command_template,
        &[("exe", &exe), ("marker", &marker)],
    );
    let mut entry: Value = serde_json::from_str(&guard.hook_entry)
        .expect("guard.hook_entry parses as JSON (proven at descriptor load)");
    substitute_strings(
        &mut entry,
        &[("matcher", &guard.matcher), ("command", &command)],
    );

    let hooks = config
        .as_object_mut()
        .expect("hook config root is a JSON object")
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let pre = hooks
        .as_object_mut()
        .expect("hooks is a JSON object")
        .entry("PreToolUse")
        .or_insert_with(|| json!([]));
    pre.as_array_mut()
        .expect("PreToolUse is an array")
        .push(entry);
    write_json(&hooks_path, &config)?;

    write_manifest(
        &skills_dir.join(GUARD_MANIFEST),
        &hooks_path,
        hooks_existed,
        backup,
        &marker_path,
    )?;

    Ok(marker_path)
}

/// Evaluate a PreToolUse hook `payload` against `marker`. Returns the deny
/// verdict to print on stdout (the descriptor's `verdict_template` with
/// `{reason}` filled), or `None` to allow (print nothing). Every error path —
/// empty/malformed payload, unrenderable template — fails open: the hook must
/// never brick a session.
pub(crate) fn guard_verdict(
    guard: &GuardSection,
    payload: &str,
    marker: Option<GuardMarker>,
) -> Option<String> {
    let (tool_name, tool_input) = parse_tool_call(payload)?;
    let decision = decide(&tool_name, &tool_input, marker.as_ref(), now_ms());
    if decision.allow {
        return None;
    }
    let mut verdict: Value = serde_json::from_str(&guard.verdict_template).ok()?;
    substitute_strings(
        &mut verdict,
        &[("reason", decision.reason.as_deref().unwrap_or(""))],
    );
    serde_json::to_string(&verdict).ok()
}

/// The hook-config dir the install created outside the skills dir, which
/// teardown prunes when restoring the original config leaves it empty.
/// Derived from the data: `hooks_file`'s parent dir, unless the hook file is
/// at the env root (nothing to prune) or the parent is the skills dir or an
/// ancestor of it (staging already owns that tree).
pub(crate) fn hook_cleanup_dir(
    guard: &GuardSection,
    skills_dir_rel: Option<&str>,
    stage_root: &Path,
) -> Option<PathBuf> {
    let (parent, _) = guard.hooks_file.rsplit_once('/')?;
    if let Some(skills) = skills_dir_rel {
        // Component-wise ancestry: `.a` owns `.a/b/skills` but not `.ab/skills`.
        let is_ancestor = skills == parent
            || skills
                .strip_prefix(parent)
                .is_some_and(|rest| rest.starts_with('/'));
        if is_ancestor {
            return None;
        }
    }
    Some(resolve_rel(stage_root, parent))
}

/// True when any *string value* in `value` contains `token`. Descriptor
/// validation uses this to prove template placeholders sit where
/// [`substitute_strings`] will actually reach them (keys are never
/// substituted).
pub(crate) fn any_string_value_contains(value: &Value, token: &str) -> bool {
    match value {
        Value::String(s) => s.contains(token),
        Value::Array(items) => items.iter().any(|v| any_string_value_contains(v, token)),
        Value::Object(map) => map.values().any(|v| any_string_value_contains(v, token)),
        _ => false,
    }
}

/// Substitute `{token}` placeholders in every string value of `value`,
/// in place. Substituting after parsing (rather than into the template text)
/// lets values carry quotes without JSON-escaping concerns — the serializer
/// escapes them, exactly as `json!` used to.
fn substitute_strings(value: &mut Value, vars: &[(&str, &str)]) {
    match value {
        Value::String(s) if s.contains('{') => {
            *s = subst(s, vars);
        }
        Value::Array(items) => {
            for item in items {
                substitute_strings(item, vars);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                substitute_strings(item, vars);
            }
        }
        _ => {}
    }
}

/// Resolve a `/`-separated descriptor-relative path under `root` — the same
/// idiom as [`super::descriptor_adapter::DescriptorAdapter::skills_dir`].
fn resolve_rel(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(root.to_path_buf(), |path, segment| path.join(segment))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::descriptor::{EMBEDDED_DESCRIPTORS, HarnessDescriptor, load_descriptor};
    use crate::sandbox::guard_is_armed;
    use crate::sandbox::install::{iso_millis, teardown_guard};
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

    /// Load one embedded descriptor by label — engine tests run against the
    /// real shipped guard data, not fixtures.
    fn descriptor(label: &str) -> HarnessDescriptor {
        let (source, toml_src) = EMBEDDED_DESCRIPTORS
            .iter()
            .find(|(path, _)| path.ends_with(&format!("{label}.toml")))
            .unwrap_or_else(|| panic!("no embedded descriptor for {label}"));
        load_descriptor(toml_src, source).unwrap()
    }

    /// Arm the guard for `label` under `stage_root` via the engine, returning
    /// the marker path.
    fn install(label: &str, stage_root: &Path) -> PathBuf {
        let d = descriptor(label);
        let skills = resolve_rel(stage_root, d.skills_dir.as_deref().unwrap());
        install_guard(
            d.guard.as_ref().unwrap(),
            &skills,
            stage_root,
            Path::new("/g/eval-magic"),
            None,
        )
        .unwrap()
    }

    fn verdict(label: &str, payload: &str, marker: Option<GuardMarker>) -> Option<String> {
        let d = descriptor(label);
        guard_verdict(d.guard.as_ref().unwrap(), payload, marker)
    }

    fn claude_skills_dir(stage_root: &Path) -> PathBuf {
        stage_root.join(".claude").join("skills")
    }

    fn settings_path(stage_root: &Path) -> PathBuf {
        stage_root.join(".claude").join("settings.local.json")
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
    fn install_writes_an_active_marker_hook_and_manifest() {
        let c = setup();
        install("claude-code", &c.stage_root);

        let marker = read_json(&claude_skills_dir(&c.stage_root).join(GUARD_MARKER));
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

        assert!(
            claude_skills_dir(&c.stage_root)
                .join(GUARD_MANIFEST)
                .exists()
        );
    }

    #[test]
    fn marker_scopes_allowed_roots_to_the_env_and_temp_only() {
        let c = setup();
        install("claude-code", &c.stage_root);

        let marker = read_json(&claude_skills_dir(&c.stage_root).join(GUARD_MARKER));
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
        let marker = install("claude-code", &c.stage_root);
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
        install("claude-code", &c.stage_root);
        assert!(settings_path(&c.stage_root).exists());

        assert!(teardown_guard(&c.stage_root));
        assert!(!settings_path(&c.stage_root).exists());
        assert!(!claude_skills_dir(&c.stage_root).join(GUARD_MARKER).exists());
        assert!(
            !claude_skills_dir(&c.stage_root)
                .join(GUARD_MANIFEST)
                .exists()
        );
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

        install("claude-code", &c.stage_root);
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
        let marker_path = claude_skills_dir(&c.stage_root).join(GUARD_MARKER);
        fs::create_dir_all(claude_skills_dir(&c.stage_root)).unwrap();

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
        fs::create_dir_all(claude_skills_dir(&c.stage_root)).unwrap();
        fs::write(claude_skills_dir(&c.stage_root).join(GUARD_MARKER), "{}").unwrap();
        assert!(teardown_guard(&c.stage_root));
        assert!(!claude_skills_dir(&c.stage_root).join(GUARD_MARKER).exists());
    }

    #[test]
    fn allows_returns_none() {
        let payload = r#"{ "tool_name": "Read", "tool_input": { "file_path": "/etc/passwd" } }"#;
        assert_eq!(verdict("claude-code", payload, Some(marker())), None);
    }

    #[test]
    fn deny_returns_pretooluse_deny_json() {
        let payload = r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#;
        let out = verdict("claude-code", payload, Some(marker())).expect("should deny");
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

    /// Byte-pin of both deny verdicts: full-string equality against the exact
    /// serialization armed hooks have always read. Key order comes from the
    /// descriptor templates and must never drift.
    #[test]
    fn deny_verdict_bytes_match_the_on_disk_contract() {
        let payload = r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#;
        assert_eq!(
            verdict("claude-code", payload, Some(marker())).expect("should deny"),
            "{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\
             \"permissionDecision\":\"deny\",\"permissionDecisionReason\":\
             \"eval guard: Write to /etc/passwd is outside the eval sandbox \
             (allowed: /work/.eval-magic)\"}}"
        );

        let payload =
            r#"{ "tool_name": "Bash", "tool_input": { "command": "npm install left-pad" } }"#;
        assert_eq!(
            verdict("codex", payload, Some(marker())).expect("should block"),
            "{\"decision\":\"block\",\"reason\":\"eval guard: blocked Bash \
             (package install/add) — runs outside the eval sandbox\"}"
        );
    }

    #[test]
    fn no_marker_allows_everything() {
        let payload = r#"{ "tool_name": "Write", "tool_input": { "file_path": "/etc/passwd" } }"#;
        assert_eq!(verdict("claude-code", payload, None), None);
    }

    #[test]
    fn empty_or_malformed_payload_fails_open() {
        assert_eq!(verdict("claude-code", "", Some(marker())), None);
        assert_eq!(verdict("claude-code", "not json", Some(marker())), None);
    }

    #[test]
    fn codex_install_writes_project_hook_marker_and_manifest() {
        let c = setup();
        install("codex", &c.stage_root);

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

        install("codex", &c.stage_root);
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
        let out = verdict("codex", payload, Some(marker())).expect("should block");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("blocked Bash"));
    }

    #[test]
    fn codex_apply_patch_outside_allowed_roots_blocks() {
        let payload = r#"{ "hook_event_name": "PreToolUse", "tool_name": "apply_patch", "tool_input": { "files": ["/etc/passwd"] } }"#;
        let out = verdict("codex", payload, Some(marker())).expect("should block");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("apply_patch"));
    }

    #[test]
    fn codex_apply_patch_inside_allowed_roots_allows() {
        let payload = r#"{ "hook_event_name": "PreToolUse", "tool_name": "apply_patch", "tool_input": { "files": ["/work/.eval-magic/out.md"] } }"#;
        assert_eq!(verdict("codex", payload, Some(marker())), None);
    }

    /// The cleanup dir is derived from the guard data: the hooks file's parent,
    /// unless the hook file sits at the env root or inside the skills dir's
    /// ancestry (staging already owns that tree).
    #[test]
    fn hook_cleanup_dir_derivation_table() {
        let root = Path::new("/env");
        let with = |hooks_file: &str| {
            let mut guard = descriptor("codex").guard.unwrap();
            guard.hooks_file = hooks_file.to_string();
            guard
        };

        // Claude: `.claude` is the skills dir's parent — staging owns it.
        assert_eq!(
            hook_cleanup_dir(
                &with(".claude/settings.local.json"),
                Some(".claude/skills"),
                root
            ),
            None
        );
        // Codex: `.codex` is created for the hook alone — prune it.
        assert_eq!(
            hook_cleanup_dir(&with(".codex/hooks.json"), Some(".agents/skills"), root),
            Some(PathBuf::from("/env/.codex"))
        );
        // Hook file at the env root: never prune the env itself.
        assert_eq!(
            hook_cleanup_dir(&with("hooks.json"), Some(".agents/skills"), root),
            None
        );
        // Parent is an ancestor of the skills dir (not merely a string prefix:
        // `.a` vs `.ab/skills` stays prunable).
        assert_eq!(
            hook_cleanup_dir(&with(".a/hooks.json"), Some(".a/b/skills"), root),
            None
        );
        assert_eq!(
            hook_cleanup_dir(&with(".a/hooks.json"), Some(".ab/skills"), root),
            Some(PathBuf::from("/env/.a"))
        );
    }
}
