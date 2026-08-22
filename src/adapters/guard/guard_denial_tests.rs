use super::*;
use crate::adapters::descriptor::{EMBEDDED_DESCRIPTORS, HarnessDescriptor, load_descriptor};
use crate::sandbox::install::teardown_guard;
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

fn descriptor(label: &str) -> HarnessDescriptor {
    let (source, toml_src) = EMBEDDED_DESCRIPTORS
        .iter()
        .find(|(path, _)| path.ends_with(&format!("{label}.toml")))
        .unwrap_or_else(|| panic!("no embedded descriptor for {label}"));
    load_descriptor(toml_src, source).unwrap()
}

fn install(label: &str, stage_root: &Path) -> PathBuf {
    let descriptor = descriptor(label);
    let skills = resolve_rel(stage_root, descriptor.skills_dir.as_deref().unwrap());
    install_guard(
        descriptor.guard.as_ref().unwrap(),
        &skills,
        stage_root,
        Path::new("/g/eval-magic"),
        None,
        &Default::default(),
    )
    .unwrap()
}

fn verdict(label: &str, payload: &str, marker: Option<GuardMarker>) -> Option<String> {
    let descriptor = descriptor(label);
    guard_verdict(descriptor.guard.as_ref().unwrap(), label, payload, marker)
}

fn marker() -> GuardMarker {
    GuardMarker {
        active: Some(true),
        allowed_roots: Some(vec!["/work/.eval-magic".to_string()]),
        expires_at: None,
        denial_log_path: None,
        guard_policy: None,
    }
}

fn codex_bash_verdict(command: &str) -> Option<String> {
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "cwd": "/work/.eval-magic",
        "tool_name": "Bash",
        "tool_input": { "command": command },
    });
    verdict("codex", &payload.to_string(), Some(marker()))
}

fn claude_skills_dir(stage_root: &Path) -> PathBuf {
    stage_root.join(".claude").join("skills")
}

fn settings_path(stage_root: &Path) -> PathBuf {
    stage_root.join(".claude").join("settings.local.json")
}

fn denial_log_path(stage_root: &Path) -> PathBuf {
    stage_root
        .join(".eval-magic-outputs")
        .join("guard-denials.jsonl")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn absolutize(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[test]
fn install_writes_an_active_marker_hook_manifest_and_empty_denial_log() {
    let case = setup();
    install("claude-code", &case.stage_root);

    let marker = read_json(&claude_skills_dir(&case.stage_root).join(GUARD_MARKER));
    assert_eq!(marker["active"], json!(true));
    let expires = marker["expiresAt"].as_str().unwrap();
    let exp_ms = DateTime::parse_from_rfc3339(expires)
        .unwrap()
        .timestamp_millis();
    assert!(exp_ms > now_ms());
    let env = absolutize(&case.stage_root).display().to_string();
    assert!(
        marker["allowedRoots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|root| root.as_str().unwrap() == env)
    );

    let settings = read_json(&settings_path(&case.stage_root));
    let hook = &settings["hooks"]["PreToolUse"][0];
    assert!(hook["matcher"].as_str().unwrap().contains("Write"));
    assert!(
        hook["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("guard")
    );
    assert!(
        claude_skills_dir(&case.stage_root)
            .join(GUARD_MANIFEST)
            .exists()
    );
    assert_eq!(
        marker["denialLogPath"],
        json!(absolutize(&denial_log_path(&case.stage_root)))
    );
    assert_eq!(
        fs::read_to_string(denial_log_path(&case.stage_root)).unwrap(),
        ""
    );
}

#[test]
fn rearming_truncates_stale_guard_denials() {
    let case = setup();
    install("codex", &case.stage_root);
    fs::write(denial_log_path(&case.stage_root), "stale denial\n").unwrap();

    install("codex", &case.stage_root);

    assert_eq!(
        fs::read_to_string(denial_log_path(&case.stage_root)).unwrap(),
        ""
    );
}

#[test]
fn teardown_preserves_guard_denial_evidence() {
    let case = setup();
    install("claude-code", &case.stage_root);
    assert!(settings_path(&case.stage_root).exists());
    fs::write(denial_log_path(&case.stage_root), "preserved evidence\n").unwrap();

    assert!(teardown_guard(&case.stage_root));
    assert!(!settings_path(&case.stage_root).exists());
    assert!(
        !claude_skills_dir(&case.stage_root)
            .join(GUARD_MARKER)
            .exists()
    );
    assert!(
        !claude_skills_dir(&case.stage_root)
            .join(GUARD_MANIFEST)
            .exists()
    );
    assert_eq!(
        fs::read_to_string(denial_log_path(&case.stage_root)).unwrap(),
        "preserved evidence\n"
    );
}

#[test]
fn codex_apply_patch_command_resolves_relative_targets_from_hook_cwd() {
    let payload = r#"{
        "hook_event_name": "PreToolUse",
        "cwd": "/work/.eval-magic/eval-root",
        "tool_name": "apply_patch",
        "tool_input": {
            "command": "*** Begin Patch\n*** Update File: fixtures/input.txt\n*** Move to: fixtures/output.txt\n*** End Patch\n"
        }
    }"#;
    assert_eq!(verdict("codex", payload, Some(marker())), None);
}

#[test]
fn codex_relative_redirection_inside_hook_cwd_allows() {
    let payload = r#"{
        "hook_event_name": "PreToolUse",
        "cwd": "/work/.eval-magic/eval-root",
        "tool_name": "Bash",
        "tool_input": { "command": "printf '%s\n' done > final-message.md" }
    }"#;
    assert_eq!(verdict("codex", payload, Some(marker())), None);
}

#[test]
fn codex_allowed_root_mention_does_not_hide_an_outside_redirection() {
    let payload = r#"{
        "hook_event_name": "PreToolUse",
        "cwd": "/work/.eval-magic/eval-root",
        "tool_name": "Bash",
        "tool_input": {
            "command": "printf '%s\n' /work/.eval-magic/eval-root > /etc/eval-magic-out"
        }
    }"#;
    let out = verdict("codex", payload, Some(marker())).expect("should block");
    assert!(
        serde_json::from_str::<Value>(&out).unwrap()["reason"]
            .as_str()
            .unwrap()
            .contains("output redirection")
    );
}

#[test]
fn codex_redirection_scanner_allows_literal_targets_inside_hook_cwd() {
    for command in [
        "printf done > /work/.eval-magic/absolute.txt",
        "printf done > fixtures/output.txt",
        "printf done > ./fixtures/output.txt",
        "printf done > .eval-magic-outputs/final-message.md",
        "printf done > final-message.md",
        "printf done 2>>\"quoted path.log\"",
        "printf done >| overwritten.txt",
        "printf done > stdout.txt 2>> stderr.txt",
        "printf done | tee \"quoted path.log\"",
        "printf done | tee -a one.txt two.txt",
        "printf done | sudo tee final-message.md",
    ] {
        assert_eq!(
            codex_bash_verdict(command),
            None,
            "inside literal target should be allowed: {command}"
        );
    }
}

/// `2>&1` and friends duplicate a file descriptor — they never open a file, so
/// they must not read as output redirection. Agents use these reflexively.
#[test]
fn codex_redirection_scanner_allows_file_descriptor_duplication() {
    for command in [
        "git status 2>&1 | head -20",
        "printf done >&2",
        "printf done 1>&2",
        "printf done >&-",
        "printf done > final-message.md 2>&1",
        "printf done | tee final-message.md 2>&1",
    ] {
        assert_eq!(
            codex_bash_verdict(command),
            None,
            "fd duplication should be allowed: {command}"
        );
    }
}

/// The null device and the standard streams are not meaningful write targets.
#[test]
fn codex_redirection_scanner_allows_non_file_devices() {
    for command in [
        "ls fixtures 2>/dev/null",
        "find . -type f 2>/dev/null | head -50",
        "printf done >/dev/null 2>&1",
        "printf done > /dev/stdout",
        "printf done > /dev/stderr",
        "printf done > /dev/fd/1",
        "printf done | tee /dev/null",
    ] {
        assert_eq!(
            codex_bash_verdict(command),
            None,
            "non-file device should be allowed: {command}"
        );
    }
}

#[test]
fn codex_redirection_scanner_denies_outside_dynamic_and_malformed_targets() {
    for command in [
        "printf done > ../outside.txt",
        "printf done > /etc/out.txt",
        "printf /work/.eval-magic > /etc/decoy.txt",
        "printf done > good.txt 2>> /etc/bad.txt",
        "printf done | tee /etc/out.txt",
        "printf done | sudo tee /etc/out.txt",
        "printf done > \"$OUT\"",
        "printf done > ${OUT}",
        "printf done > \"unterminated",
        // A dynamic target stays denied even when an fd duplication follows it:
        // nothing resolved, so nothing was proven in bounds.
        "printf done > \"$OUT\" 2>&1",
        // Only the null device itself is exempt — not every path under /dev,
        // and not a path that merely starts with the same bytes.
        "printf done > /dev/nullx",
        "printf done > /dev/sda",
        // `..` is popped during resolution, so a /dev prefix cannot launder an
        // out-of-bounds target.
        "printf done > /dev/../etc/passwd",
        "printf done 2>/etc/err.log",
    ] {
        let out = codex_bash_verdict(command)
            .unwrap_or_else(|| panic!("unsafe target should be denied: {command}"));
        assert!(
            serde_json::from_str::<Value>(&out).unwrap()["reason"]
                .as_str()
                .unwrap()
                .contains("output redirection"),
            "{command}: {out}"
        );
    }
}

#[test]
fn codex_apply_patch_validates_move_source_and_destination() {
    for command in [
        "*** Begin Patch\n*** Update File: /etc/source.txt\n*** Move to: fixtures/output.txt\n*** End Patch\n",
        "*** Begin Patch\n*** Update File: fixtures/source.txt\n*** Move to: /etc/output.txt\n*** End Patch\n",
    ] {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "cwd": "/work/.eval-magic",
            "tool_name": "apply_patch",
            "tool_input": { "command": command },
        });
        assert!(
            verdict("codex", &payload.to_string(), Some(marker())).is_some(),
            "every move endpoint must stay inside the allowed root"
        );
    }
}

#[test]
fn guard_denials_append_privacy_safe_sorted_metadata_and_allows_do_not_log() {
    let tmp = TempDir::new().unwrap();
    let eval_root = absolutize(&tmp.path().join("eval-root"));
    fs::create_dir_all(&eval_root).unwrap();
    let log_path = tmp.path().join("guard-denials.jsonl");
    fs::write(&log_path, "").unwrap();
    let marker: GuardMarker = serde_json::from_value(json!({
        "active": true,
        "allowedRoots": [eval_root],
        "denialLogPath": absolutize(&log_path),
    }))
    .unwrap();

    let allowed = json!({
        "hook_event_name": "PreToolUse",
        "cwd": eval_root,
        "tool_name": "Bash",
        "tool_input": { "command": "printf done > final-message.md" },
    });
    assert_eq!(
        verdict("codex", &allowed.to_string(), Some(marker.clone())),
        None
    );
    assert_eq!(fs::read_to_string(&log_path).unwrap(), "");

    let denied = json!({
        "hook_event_name": "PreToolUse",
        "cwd": eval_root,
        "tool_name": "Bash",
        "tool_input": {
            "z": "not persisted",
            "command": "printf secret > /etc/eval-magic-out",
            "a": 1,
        },
    });
    assert!(verdict("codex", &denied.to_string(), Some(marker.clone())).is_some());
    let second_denial = json!({
        "hook_event_name": "PreToolUse",
        "cwd": eval_root,
        "tool_name": "Write",
        "tool_input": { "file_path": "/etc/second-out" },
    });
    assert!(verdict("codex", &second_denial.to_string(), Some(marker)).is_some());

    let raw = fs::read_to_string(&log_path).unwrap();
    let lines = raw.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    let record: Value = serde_json::from_str(lines[0]).unwrap();
    assert!(record["timestamp"].as_str().is_some());
    assert_eq!(record["harness"], "codex");
    assert_eq!(record["tool"], "Bash");
    assert!(
        record["reason"]
            .as_str()
            .unwrap()
            .contains("output redirection")
    );
    assert_eq!(record["resolved_targets"], json!(["/etc/eval-magic-out"]));
    assert_eq!(record["input_keys"], json!(["a", "command", "z"]));
    assert!(record.get("command").is_none(), "{record}");
    let second: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(second["tool"], "Write");
    assert_eq!(second["resolved_targets"], json!(["/etc/second-out"]));
    assert_eq!(second["input_keys"], json!(["file_path"]));
    assert!(!raw.contains("secret"), "{raw}");
    assert!(!raw.contains("not persisted"), "{raw}");
}

#[test]
fn guard_denial_logging_failure_preserves_the_block_verdict() {
    let tmp = TempDir::new().unwrap();
    let marker: GuardMarker = serde_json::from_value(json!({
        "active": true,
        "allowedRoots": [tmp.path()],
        "denialLogPath": tmp.path(),
    }))
    .unwrap();
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "cwd": tmp.path(),
        "tool_name": "Write",
        "tool_input": { "file_path": "/etc/passwd" },
    });

    assert!(
        verdict("codex", &payload.to_string(), Some(marker)).is_some(),
        "logging errors must not turn a denial into an allow"
    );
}
