//! `cline-plugin` guard-engine tests: the staged plugin file's byte-exact
//! contract, install/teardown semantics, and deny-verdict rendering against
//! the real embedded descriptor data.

use super::*;
use crate::adapters::descriptor::{EMBEDDED_DESCRIPTORS, HarnessDescriptor, load_descriptor};
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

/// Load one embedded descriptor by label — engine tests run against the real
/// shipped guard data, not fixtures.
fn descriptor(label: &str) -> HarnessDescriptor {
    let (source, toml_src) = EMBEDDED_DESCRIPTORS
        .iter()
        .find(|(path, _)| path.ends_with(&format!("{label}.toml")))
        .unwrap_or_else(|| panic!("no embedded descriptor for {label}"));
    load_descriptor(toml_src, source).unwrap()
}

/// Arm the guard for `label` under `stage_root` via the engine, returning the
/// marker path.
fn install(label: &str, stage_root: &Path) -> PathBuf {
    let d = descriptor(label);
    let skills = resolve_rel(stage_root, d.skills_dir.as_deref().unwrap());
    install_guard(
        d.guard.as_ref().unwrap(),
        &skills,
        stage_root,
        Path::new("/g/eval-magic"),
        None,
        &Default::default(),
    )
    .unwrap()
}

fn verdict(label: &str, payload: &str, marker: Option<GuardMarker>) -> Option<String> {
    let d = descriptor(label);
    guard_verdict(d.guard.as_ref().unwrap(), label, payload, marker)
}

/// A live marker (active, no expiry → unexpired) scoped to one root.
fn marker() -> GuardMarker {
    GuardMarker {
        active: Some(true),
        allowed_roots: Some(vec!["/work/.eval-magic".to_string()]),
        expires_at: None,
        denial_log_path: None,
        guard_policy: None,
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn absolutize(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn cline_skills_dir(stage_root: &Path) -> PathBuf {
    stage_root.join(".cline").join("skills")
}

/// The staged plugin is a *directory* holding one embedded `index.js`: Cline
/// auto-loads project plugin dirs from `.cline/plugins/` (loose files are
/// ignored) — 3.0.53 spike-verified, docs/cline-notes.md.
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
/// `{exe}`/`{marker}` substituted as JSON string literals. Written out here
/// in full so any template edit forces a reviewed re-pin — the file is the
/// on-disk contract armed envs run.
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
/// `guard-hook` rendering, so this characterizes the shape the staged plugin
/// parses (`decision`/`reason`) against the real descriptor data.
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
/// `command` before forwarding; this is the payload shape it sends, and the
/// arbiter's shell patterns must classify it.
#[test]
fn cline_deny_verdict_classifies_a_joined_shell_command() {
    let payload = r#"{ "tool_name": "run_commands", "cwd": "/work/.eval-magic", "tool_input": { "command": "npm install --prefix /outside left-pad" } }"#;
    let verdict = verdict("cline", payload, Some(marker())).expect("should block");
    assert!(verdict.contains("package install/add"), "{verdict}");
}

#[test]
fn cline_allows_an_in_bounds_write() {
    let payload =
        r#"{ "tool_name": "editor", "tool_input": { "path": "/work/.eval-magic/out.md" } }"#;
    assert_eq!(verdict("cline", payload, Some(marker())), None);
}
