//! The write-guard engine: install + verdict rendered from a descriptor's
//! `[guard]` data block, with the install side selected by the block's
//! `engine` discriminator.
//!
//! `json-hooks` (Claude Code, Codex) merges one PreToolUse hook entry (the
//! descriptor's `hook_entry` JSON template) into the harness's hook-config
//! file (`hooks_file`). `opencode-plugin` stages an embedded JS project
//! plugin whole at `plugin_file` — OpenCode auto-loads project plugins by
//! directory convention, and the plugin blocks a tool call by throwing the
//! deny verdict's reason. Both arms stage the marker/manifest via
//! [`crate::sandbox::install`]. The verdict side is shared: it feeds a hook
//! payload through the shared arbiter ([`crate::sandbox::decide`]) and
//! serializes the descriptor's `verdict_template` on deny. Template key order
//! is authored in the descriptor and serialized verbatim (`serde_json` keeps
//! insertion order), so verdict bytes and hook-file shape are pinned by data,
//! not code.
//!
//! Guard blocks exist only in embedded built-in descriptors — the guard fails
//! open, so [`super::descriptor::layers::check_user_layer_restrictions`] bars
//! user layers from declaring one.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::core::fs::write_json;
use crate::sandbox::decide::{GuardMarker, decide_with_cwd};
use crate::sandbox::install::{
    GUARD_MANIFEST, GUARD_MARKER, iso_millis, write_manifest, write_marker,
};
use crate::sandbox::{GuardDenialRecord, now_ms, parse_tool_call};

use super::descriptor::{GuardEngine, GuardSection, subst};

/// Arm the write guard: marker + manifest under `skills_dir` (absolute,
/// resolved by the caller), then the engine-specific hook surface. Returns
/// the staged marker path.
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

    match guard.engine {
        GuardEngine::JsonHooks => {
            install_json_hooks(guard, skills_dir, stage_root, guard_exe, &marker_path)
        }
        GuardEngine::OpencodePlugin => {
            install_opencode_plugin(guard, skills_dir, stage_root, guard_exe, &marker_path)
        }
        GuardEngine::ClinePlugin => {
            install_cline_plugin(guard, skills_dir, stage_root, guard_exe, &marker_path)
        }
    }
}

/// The embedded OpenCode project plugin. The `{exe}`/`{marker}` placeholders
/// substitute as JSON string literals (a JSON string is a valid JS string
/// literal), so any exe/marker path characters survive without hand-escaping.
/// The file's exact bytes are pinned in this module's tests — the staged
/// plugin is an on-disk contract.
const OPENCODE_GUARD_PLUGIN_TEMPLATE: &str =
    include_str!("../../harnesses/opencode-guard-plugin.js");

/// The opencode-plugin arm: the embedded JS template with `{exe}`/`{marker}`
/// substituted, staged whole at the descriptor's `plugin_file`. No merge —
/// OpenCode auto-loads project plugins by directory convention, and the
/// plugin blocks by throwing, so the file *is* the hook surface.
fn install_opencode_plugin(
    guard: &GuardSection,
    skills_dir: &Path,
    stage_root: &Path,
    guard_exe: &Path,
    marker_path: &Path,
) -> io::Result<PathBuf> {
    let plugin_path = resolve_rel(
        stage_root,
        guard
            .plugin_file
            .as_deref()
            .expect("guard.plugin_file is declared (proven at descriptor load)"),
    );
    if let Some(parent) = plugin_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let plugin_existed = plugin_path.exists();
    let backup = if plugin_existed {
        Some(fs::read_to_string(&plugin_path)?)
    } else {
        None
    };

    let exe = serde_json::to_string(&guard_exe.display().to_string())
        .expect("a path string serializes as JSON");
    let marker = serde_json::to_string(&marker_path.display().to_string())
        .expect("a path string serializes as JSON");
    let plugin = subst(
        OPENCODE_GUARD_PLUGIN_TEMPLATE,
        &[("exe", &exe), ("marker", &marker)],
    );
    fs::write(&plugin_path, plugin)?;

    write_manifest(
        &skills_dir.join(GUARD_MANIFEST),
        &plugin_path,
        plugin_existed,
        backup,
        marker_path,
    )?;

    Ok(marker_path.to_path_buf())
}

/// The embedded Cline project plugin. The `{exe}`/`{marker}` placeholders
/// substitute as JSON string literals (a JSON string is a valid JS string
/// literal), so any exe/marker path characters survive without hand-escaping.
/// The file's exact bytes are pinned in this module's tests — the staged
/// plugin is an on-disk contract.
const CLINE_GUARD_PLUGIN_TEMPLATE: &str = include_str!("../../harnesses/cline-guard-plugin.js");

/// The cline-plugin arm: the embedded JS template with `{exe}`/`{marker}`
/// substituted, staged whole at the descriptor's `plugin_file`. Cline
/// auto-loads project plugin *directories* from `.cline/plugins/` (a bare
/// `index.js` is discovered without a package.json; loose files at the
/// plugins root are ignored — 3.0.53 spike-verified), and the plugin's
/// `beforeTool` hook blocks by returning `{skip: true, reason}`, so the file
/// *is* the hook surface.
fn install_cline_plugin(
    guard: &GuardSection,
    skills_dir: &Path,
    stage_root: &Path,
    guard_exe: &Path,
    marker_path: &Path,
) -> io::Result<PathBuf> {
    let plugin_path = resolve_rel(
        stage_root,
        guard
            .plugin_file
            .as_deref()
            .expect("guard.plugin_file is declared (proven at descriptor load)"),
    );
    if let Some(parent) = plugin_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let plugin_existed = plugin_path.exists();
    let backup = if plugin_existed {
        Some(fs::read_to_string(&plugin_path)?)
    } else {
        None
    };

    let exe = serde_json::to_string(&guard_exe.display().to_string())
        .expect("a path string serializes as JSON");
    let marker = serde_json::to_string(&marker_path.display().to_string())
        .expect("a path string serializes as JSON");
    let plugin = subst(
        CLINE_GUARD_PLUGIN_TEMPLATE,
        &[("exe", &exe), ("marker", &marker)],
    );
    fs::write(&plugin_path, plugin)?;

    write_manifest(
        &skills_dir.join(GUARD_MANIFEST),
        &plugin_path,
        plugin_existed,
        backup,
        marker_path,
    )?;

    Ok(marker_path.to_path_buf())
}

/// The json-hooks arm: one hook entry merged into the descriptor's
/// `hooks_file`.
fn install_json_hooks(
    guard: &GuardSection,
    skills_dir: &Path,
    stage_root: &Path,
    guard_exe: &Path,
    marker_path: &Path,
) -> io::Result<PathBuf> {
    let hooks_path = resolve_rel(
        stage_root,
        guard
            .hooks_file
            .as_deref()
            .expect("guard.hooks_file is declared (proven at descriptor load)"),
    );
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
        guard
            .command_template
            .as_deref()
            .expect("guard.command_template is declared (proven at descriptor load)"),
        &[("exe", &exe), ("marker", &marker)],
    );
    let mut entry: Value = serde_json::from_str(
        guard
            .hook_entry
            .as_deref()
            .expect("guard.hook_entry is declared (proven at descriptor load)"),
    )
    .expect("guard.hook_entry parses as JSON (proven at descriptor load)");
    substitute_strings(
        &mut entry,
        &[
            (
                "matcher",
                guard
                    .matcher
                    .as_deref()
                    .expect("guard.matcher is declared (proven at descriptor load)"),
            ),
            ("command", &command),
        ],
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
        marker_path,
    )?;

    Ok(marker_path.to_path_buf())
}

/// Evaluate a PreToolUse hook `payload` against `marker`. Returns the deny
/// verdict to print on stdout (the descriptor's `verdict_template` with
/// `{reason}` filled), or `None` to allow (print nothing). Every error path —
/// empty/malformed payload, unrenderable template — fails open: the hook must
/// never brick a session.
pub(crate) fn guard_verdict(
    guard: &GuardSection,
    harness: &str,
    payload: &str,
    marker: Option<GuardMarker>,
) -> Option<String> {
    let call = parse_tool_call(payload)?;
    let process_cwd = std::env::current_dir().unwrap_or_default();
    let timestamp_ms = now_ms();
    let evaluation = decide_with_cwd(
        &call.tool_name,
        &call.tool_input,
        marker.as_ref(),
        timestamp_ms,
        call.cwd.as_deref().unwrap_or(&process_cwd),
    );
    if evaluation.decision.allow {
        return None;
    }
    let reason = evaluation.decision.reason.unwrap_or_default();
    if let Some(log_path) = marker.as_ref().and_then(|m| m.denial_log_path.as_deref()) {
        let mut input_keys = call
            .tool_input
            .as_object()
            .map(|input| input.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        input_keys.sort();
        let record = GuardDenialRecord {
            timestamp: iso_millis(timestamp_ms),
            harness: harness.to_string(),
            tool: call.tool_name,
            reason: reason.clone(),
            resolved_targets: evaluation.resolved_targets,
            input_keys,
        };
        // Observability must never weaken enforcement: a missing/unwritable log
        // still returns the original block verdict.
        let _ = append_guard_denial(Path::new(log_path), &record);
    }
    let mut verdict: Value = serde_json::from_str(&guard.verdict_template).ok()?;
    substitute_strings(&mut verdict, &[("reason", &reason)]);
    serde_json::to_string(&verdict).ok()
}

fn append_guard_denial(path: &Path, record: &GuardDenialRecord) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let mut line = serde_json::to_vec(record)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    line.push(b'\n');
    file.write_all(&line)
}

/// The hook-surface dir the install created outside the skills dir, which
/// teardown prunes when restoring the original file leaves it empty. Derived
/// from the data: the engine's staged file's parent dir (`hooks_file` for
/// json-hooks, `plugin_file` for opencode-plugin), unless the file is at the
/// env root (nothing to prune) or the parent is the skills dir or an ancestor
/// of it (staging already owns that tree).
pub(crate) fn hook_cleanup_dir(
    guard: &GuardSection,
    skills_dir_rel: Option<&str>,
    stage_root: &Path,
) -> Option<PathBuf> {
    let hook_file = match guard.engine {
        GuardEngine::JsonHooks => guard.hooks_file.as_deref(),
        GuardEngine::OpencodePlugin | GuardEngine::ClinePlugin => guard.plugin_file.as_deref(),
    }?;
    let (parent, _) = hook_file.rsplit_once('/')?;
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
mod guard_denial_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::descriptor::{EMBEDDED_DESCRIPTORS, HarnessDescriptor, load_descriptor};
    use crate::sandbox::guard_is_armed;
    use crate::sandbox::install::{iso_millis, teardown_guard};
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
        guard_verdict(d.guard.as_ref().unwrap(), label, payload, marker)
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
            denial_log_path: None,
        }
    }

    #[test]
    fn marker_scopes_allowed_roots_to_the_env_only() {
        let c = setup();
        install("claude-code", &c.stage_root);

        let marker = read_json(&claude_skills_dir(&c.stage_root).join(GUARD_MARKER));
        let roots: Vec<String> = marker["allowedRoots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_str().unwrap().to_string())
            .collect();

        // The guard boundary is exactly the isolated env (stage_root) — nothing
        // above it, including the host temp directory. The parent workspace tree
        // must NOT be an allowed root, or the agent could write into sibling
        // iterations / the meta dir above `env/`.
        let env = absolutize(&c.stage_root).display().to_string();
        assert_eq!(roots, vec![env]);
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
             (allowed: /work/.eval-magic). For temporary or scratch files, use \
             /work/.eval-magic/tmp.\"}}"
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

    /// The cleanup dir is derived from the guard data: the hook surface file's
    /// parent, unless the file sits at the env root or inside the skills dir's
    /// ancestry (staging already owns that tree).
    #[test]
    fn hook_cleanup_dir_derivation_table() {
        let root = Path::new("/env");
        let with = |hooks_file: &str| {
            let mut guard = descriptor("codex").guard.unwrap();
            guard.hooks_file = Some(hooks_file.to_string());
            guard
        };
        let with_plugin = |plugin_file: &str| {
            let mut guard = descriptor("opencode").guard.unwrap();
            guard.plugin_file = Some(plugin_file.to_string());
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

        // OpenCode: `.opencode/plugins` is created for the plugin alone (the
        // skills dir is its sibling, not inside it) — prune it.
        assert_eq!(
            hook_cleanup_dir(
                &with_plugin(".opencode/plugins/slow-powers-eval-guard.js"),
                Some(".opencode/skills"),
                root
            ),
            Some(PathBuf::from("/env/.opencode/plugins"))
        );
        // A plugin file directly under the skills dir's ancestor stays.
        assert_eq!(
            hook_cleanup_dir(
                &with_plugin(".opencode/guard.js"),
                Some(".opencode/skills"),
                root
            ),
            None
        );
        // Plugin file at the env root: never prune the env itself.
        assert_eq!(
            hook_cleanup_dir(&with_plugin("guard.js"), Some(".opencode/skills"), root),
            None
        );
    }

    // ── opencode-plugin engine ────────────────────────────────────────────

    fn opencode_skills_dir(stage_root: &Path) -> PathBuf {
        stage_root.join(".opencode").join("skills")
    }

    fn opencode_plugin_path(stage_root: &Path) -> PathBuf {
        stage_root
            .join(".opencode")
            .join("plugins")
            .join("slow-powers-eval-guard.js")
    }

    /// The staged plugin file, byte-for-byte: the embedded template with
    /// `{exe}`/`{marker}` substituted as JSON string literals. Written out
    /// here in full so any template edit forces a reviewed re-pin — the file
    /// is the on-disk contract armed envs run.
    const EXPECTED_PLUGIN_TEMPLATE: &str = r#"// slow-powers eval write guard — staged by `eval-magic` into this env's
// project plugins; removed by `eval-magic teardown-guard` (or the next run).
// Do not edit: re-staging overwrites, and teardown restores the original.
//
// Dumb forwarder by design: every tool call goes to
// `eval-magic guard-hook --harness opencode <marker>` on stdin and the shared
// arbiter inside the binary classifies it. Empty stdout allows; non-empty
// stdout is the deny verdict JSON whose reason blocks the call.
import { spawnSync } from "node:child_process";

const EXE = {exe};
const MARKER = {marker};

export const SlowPowersEvalGuard = async () => {
  return {
    "tool.execute.before": async (input, output) => {
      const payload = JSON.stringify({
        tool_name: input.tool,
        tool_input: output?.args ?? {},
      });
      const result = spawnSync(EXE, ["guard-hook", "--harness", "opencode", MARKER], {
        input: payload,
        encoding: "utf8",
        timeout: 10000,
        stdio: ["pipe", "pipe", "ignore"],
      });
      const stdout = (result.stdout ?? "").trim();
      if (!stdout) {
        return; // allow — also the fail-open path on spawn error or timeout
      }
      let reason = stdout;
      try {
        const verdict = JSON.parse(stdout);
        if (typeof verdict?.reason === "string") {
          reason = verdict.reason;
        }
      } catch {
        // Not the verdict shape — surface the raw stdout as the reason.
      }
      throw new Error(reason);
    },
  };
};
"#;

    fn expected_plugin(marker_path: &Path) -> String {
        let exe = serde_json::to_string("/g/eval-magic").unwrap();
        let marker = serde_json::to_string(&marker_path.display().to_string()).unwrap();
        subst(
            EXPECTED_PLUGIN_TEMPLATE,
            &[("exe", &exe), ("marker", &marker)],
        )
    }

    #[test]
    fn opencode_install_stages_the_byte_exact_plugin_marker_and_manifest() {
        let c = setup();
        let marker_path = install("opencode", &c.stage_root);

        let marker = read_json(&opencode_skills_dir(&c.stage_root).join(GUARD_MARKER));
        assert_eq!(marker["active"], json!(true));
        let env = absolutize(&c.stage_root).display().to_string();
        assert!(
            marker["allowedRoots"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.as_str().unwrap() == env)
        );

        let plugin = fs::read_to_string(opencode_plugin_path(&c.stage_root)).unwrap();
        assert_eq!(plugin, expected_plugin(&marker_path));
        // The substitution lands the generic guard-hook entry point with the
        // opencode harness and the staged marker path.
        assert!(plugin.contains("\"guard-hook\""), "{plugin}");
        assert!(plugin.contains("\"opencode\""), "{plugin}");

        assert!(
            opencode_skills_dir(&c.stage_root)
                .join(GUARD_MANIFEST)
                .exists()
        );
    }

    #[test]
    fn opencode_teardown_removes_the_plugin_and_prunes_the_plugins_dir() {
        let c = setup();
        install("opencode", &c.stage_root);
        assert!(opencode_plugin_path(&c.stage_root).exists());

        assert!(teardown_guard(&c.stage_root));
        assert!(!opencode_plugin_path(&c.stage_root).exists());
        assert!(
            !c.stage_root.join(".opencode").join("plugins").exists(),
            "the dir created for the plugin alone is pruned"
        );
        assert!(
            !opencode_skills_dir(&c.stage_root)
                .join(GUARD_MARKER)
                .exists()
        );
        assert!(
            !opencode_skills_dir(&c.stage_root)
                .join(GUARD_MANIFEST)
                .exists()
        );
    }

    #[test]
    fn opencode_teardown_restores_a_pre_existing_plugin_verbatim() {
        let c = setup();
        fs::create_dir_all(c.stage_root.join(".opencode").join("plugins")).unwrap();
        let original = "// the user's own plugin\nexport const Theirs = async () => ({});\n";
        fs::write(opencode_plugin_path(&c.stage_root), original).unwrap();

        install("opencode", &c.stage_root);
        assert!(
            fs::read_to_string(opencode_plugin_path(&c.stage_root))
                .unwrap()
                .contains("SlowPowersEvalGuard")
        );

        teardown_guard(&c.stage_root);
        assert_eq!(
            fs::read_to_string(opencode_plugin_path(&c.stage_root)).unwrap(),
            original
        );
    }

    /// Byte-pin of the opencode deny verdict: the verdict path is the shared
    /// `guard-hook` rendering, so this characterizes the shape the staged
    /// plugin parses (`decision`/`reason`) against the real descriptor data.
    #[test]
    fn opencode_deny_verdict_bytes_match_the_on_disk_contract() {
        let payload = r#"{ "tool_name": "write", "tool_input": { "filePath": "/etc/passwd" } }"#;
        assert_eq!(
            verdict("opencode", payload, Some(marker())).expect("should block"),
            "{\"decision\":\"block\",\"reason\":\"eval guard: write to /etc/passwd is \
             outside the eval sandbox (allowed: /work/.eval-magic). For temporary or scratch \
             files, use /work/.eval-magic/tmp.\"}"
        );
    }

    #[test]
    fn opencode_allows_an_in_bounds_write() {
        let payload =
            r#"{ "tool_name": "write", "tool_input": { "filePath": "/work/.eval-magic/out.md" } }"#;
        assert_eq!(verdict("opencode", payload, Some(marker())), None);
    }

    // ── cline-plugin engine ─────────────────────────────────────────────────

    fn cline_skills_dir(stage_root: &Path) -> PathBuf {
        stage_root.join(".cline").join("skills")
    }

    /// The staged plugin is a *directory* holding one embedded `index.js`:
    /// Cline auto-loads project plugin dirs from `.cline/plugins/` (loose
    /// files are ignored) — 3.0.53 spike-verified, docs/cline-notes.md.
    fn cline_plugin_path(stage_root: &Path) -> PathBuf {
        stage_root
            .join(".cline")
            .join("plugins")
            .join("slow-powers-eval-guard")
            .join("index.js")
    }

    fn expected_cline_plugin(marker_path: &Path) -> String {
        let exe = serde_json::to_string("/g/eval-magic").unwrap();
        let marker = serde_json::to_string(&marker_path.display().to_string()).unwrap();
        subst(
            EXPECTED_CLINE_PLUGIN_TEMPLATE,
            &[("exe", &exe), ("marker", &marker)],
        )
    }

    /// The staged plugin file, byte-for-byte: the embedded template with
    /// `{exe}`/`{marker}` substituted as JSON string literals. Written out
    /// here in full so any template edit forces a reviewed re-pin — the file
    /// is the on-disk contract armed envs run.
    const EXPECTED_CLINE_PLUGIN_TEMPLATE: &str = r#"// slow-powers eval write guard — staged by `eval-magic` into this env's
// project plugins; removed by `eval-magic teardown-guard` (or the next run).
// Do not edit: re-staging overwrites, and teardown restores the original.
//
// Dumb forwarder by design: every tool call goes to
// `eval-magic guard-hook --harness cline <marker>` on stdin and the shared
// arbiter inside the binary classifies it. Empty stdout allows; non-empty
// stdout is the deny verdict JSON whose reason blocks the call.
import { spawnSync } from "node:child_process";

const EXE = {exe};
const MARKER = {marker};

// Cline's plugin hook surface (3.0.53): the runtime calls `beforeTool` with
// {snapshot, tool, toolCall, input}; returning {skip: true, reason} blocks
// the call and the reason reaches the agent (and the transcript).
const SlowPowersEvalGuard = {
  name: "slow-powers-eval-guard",
  manifest: { capabilities: ["hooks"] },
  hooks: {
    beforeTool(context) {
      const name = context?.toolCall?.toolName ?? "";
      const input = context?.toolCall?.input ?? context?.input ?? {};
      // run_commands nests its shell commands as an array; the shared arbiter
      // classifies one `command` string, so join before forwarding.
      let toolInput = input;
      if (name === "run_commands" && Array.isArray(input?.commands)) {
        const { commands, ...rest } = input;
        toolInput = { ...rest, command: commands.join("\n") };
      }
      const payload = JSON.stringify({ tool_name: name, tool_input: toolInput });
      const result = spawnSync(EXE, ["guard-hook", "--harness", "cline", MARKER], {
        input: payload,
        encoding: "utf8",
        // Under the runtime's 3000ms hook budget, so a hung arbiter fails
        // open here rather than erroring the hook.
        timeout: 2000,
        stdio: ["pipe", "pipe", "ignore"],
      });
      const stdout = (result.stdout ?? "").trim();
      if (!stdout) {
        return {}; // allow — also the fail-open path on spawn error or timeout
      }
      let reason = stdout;
      try {
        const verdict = JSON.parse(stdout);
        if (typeof verdict?.reason === "string") {
          reason = verdict.reason;
        }
      } catch {
        // Not the verdict shape — surface the raw stdout as the reason.
      }
      return { skip: true, reason };
    },
  },
};

export default SlowPowersEvalGuard;
"#;

    #[test]
    fn cline_install_stages_the_byte_exact_plugin_marker_and_manifest() {
        let c = setup();
        let marker_path = install("cline", &c.stage_root);

        let marker = read_json(&cline_skills_dir(&c.stage_root).join(GUARD_MARKER));
        assert_eq!(marker["active"], json!(true));
        let env = absolutize(&c.stage_root).display().to_string();
        assert!(
            marker["allowedRoots"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.as_str().unwrap() == env)
        );

        let plugin = fs::read_to_string(cline_plugin_path(&c.stage_root)).unwrap();
        assert_eq!(plugin, expected_cline_plugin(&marker_path));
        // The substitution lands the generic guard-hook entry point with the
        // cline harness and the staged marker path.
        assert!(plugin.contains("\"guard-hook\""), "{plugin}");
        assert!(plugin.contains("\"cline\""), "{plugin}");

        assert!(
            cline_skills_dir(&c.stage_root)
                .join(GUARD_MANIFEST)
                .exists()
        );
    }

    #[test]
    fn cline_teardown_removes_the_plugin_and_prunes_the_plugin_dir() {
        let c = setup();
        install("cline", &c.stage_root);
        assert!(cline_plugin_path(&c.stage_root).exists());

        assert!(teardown_guard(&c.stage_root));
        assert!(!cline_plugin_path(&c.stage_root).exists());
        assert!(
            !c.stage_root
                .join(".cline")
                .join("plugins")
                .join("slow-powers-eval-guard")
                .exists(),
            "the dir created for the plugin alone is pruned"
        );
        assert!(!cline_skills_dir(&c.stage_root).join(GUARD_MARKER).exists());
        assert!(
            !cline_skills_dir(&c.stage_root)
                .join(GUARD_MANIFEST)
                .exists()
        );
    }

    #[test]
    fn cline_teardown_restores_a_pre_existing_plugin_verbatim() {
        let c = setup();
        let plugin_dir = c
            .stage_root
            .join(".cline")
            .join("plugins")
            .join("slow-powers-eval-guard");
        fs::create_dir_all(&plugin_dir).unwrap();
        let original = "// the user's own plugin\nexport default {};\n";
        fs::write(cline_plugin_path(&c.stage_root), original).unwrap();

        install("cline", &c.stage_root);
        assert!(
            fs::read_to_string(cline_plugin_path(&c.stage_root))
                .unwrap()
                .contains("SlowPowersEvalGuard")
        );

        teardown_guard(&c.stage_root);
        assert_eq!(
            fs::read_to_string(cline_plugin_path(&c.stage_root)).unwrap(),
            original
        );
    }

    /// Byte-pin of the cline deny verdict: the verdict path is the shared
    /// `guard-hook` rendering, so this characterizes the shape the staged
    /// plugin parses (`decision`/`reason`) against the real descriptor data.
    #[test]
    fn cline_deny_verdict_bytes_match_the_on_disk_contract() {
        let payload = r#"{ "tool_name": "editor", "tool_input": { "path": "/etc/passwd" } }"#;
        assert_eq!(
            verdict("cline", payload, Some(marker())).expect("should block"),
            "{\"decision\":\"block\",\"reason\":\"eval guard: editor to /etc/passwd is \
             outside the eval sandbox (allowed: /work/.eval-magic). For temporary or scratch \
             files, use /work/.eval-magic/tmp.\"}"
        );
    }

    /// The staged plugin joins `run_commands`' `commands` array into one
    /// `command` before forwarding; this is the payload shape it sends, and
    /// the arbiter's shell patterns must classify it.
    #[test]
    fn cline_deny_verdict_classifies_a_joined_shell_command() {
        let payload = r#"{ "tool_name": "run_commands", "tool_input": { "command": "npm install left-pad" } }"#;
        let verdict = verdict("cline", payload, Some(marker())).expect("should block");
        assert!(verdict.contains("package install/add"), "{verdict}");
    }

    #[test]
    fn cline_allows_an_in_bounds_write() {
        let payload =
            r#"{ "tool_name": "editor", "tool_input": { "path": "/work/.eval-magic/out.md" } }"#;
        assert_eq!(verdict("cline", payload, Some(marker())), None);
    }
}
