use super::{MINIMAL, err_of, load_descriptor};

/// A guard-wired descriptor whose tool vocabulary covers its matcher; the
/// base for guard/matcher mutation tests.
const GUARDED: &str = r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[run]
supports_guard = true

[tools]
write = ["Edit", "MultiEdit", "NotebookEdit", "Write"]
shell = ["Bash"]

[guard]
hooks_file = ".demo/hooks.json"
matcher = "Write|Edit|MultiEdit|NotebookEdit|Bash"
command_template = '"{exe}" guard-hook --harness demo "{marker}"'
hook_entry = '{"matcher":"{matcher}","hooks":[{"type":"command","command":"{command}"}]}'
verdict_template = '{"decision":"block","reason":"{reason}"}'
armed_message = "guard armed"
"#;

#[test]
fn rejects_guard_support_without_guard_table() {
    let err = err_of(&format!("{MINIMAL}\n[run]\nsupports_guard = true\n"));
    assert!(err.contains("run.supports_guard"), "{err}");
    assert!(err.contains("lockstep"), "{err}");
}

#[test]
fn rejects_guard_table_without_guard_support() {
    let err = err_of(&GUARDED.replace("supports_guard = true", "supports_guard = false"));
    assert!(err.contains("run.supports_guard"), "{err}");
    assert!(err.contains("lockstep"), "{err}");
}

#[test]
fn rejects_guard_matcher_tool_missing_from_vocabulary() {
    // The matcher hooks Write|Edit|MultiEdit|NotebookEdit|Bash; drop Bash
    // from the shell vocabulary and the arbiter would wave it through.
    let err = err_of(&GUARDED.replace("shell = [\"Bash\"]", "shell = [\"Shell\"]"));
    assert!(err.contains("Bash"), "{err}");
    assert!(err.contains("[tools]"), "{err}");
}

#[test]
fn rejects_hook_entry_that_is_not_json() {
    let err = err_of(&GUARDED.replace(
        r#"hook_entry = '{"matcher":"{matcher}","hooks":[{"type":"command","command":"{command}"}]}'"#,
        "hook_entry = 'not json'",
    ));
    assert!(err.contains("guard.hook_entry"), "{err}");
    assert!(err.contains("JSON"), "{err}");
}

#[test]
fn rejects_hook_entry_missing_a_placeholder() {
    // {command} in a JSON *key* must not count: only string values are
    // substituted, so a key-side placeholder would render an inert hook.
    for (mutated, needle) in [
        (
            r#"hook_entry = '{"matcher":"{matcher}","hooks":[{"type":"command","{command}":"x"}]}'"#,
            "{command}",
        ),
        (
            r#"hook_entry = '{"matcher":"Write","hooks":[{"type":"command","command":"{command}"}]}'"#,
            "{matcher}",
        ),
    ] {
        let err = err_of(&GUARDED.replace(
            r#"hook_entry = '{"matcher":"{matcher}","hooks":[{"type":"command","command":"{command}"}]}'"#,
            mutated,
        ));
        assert!(err.contains("guard.hook_entry"), "{err}");
        assert!(err.contains(needle), "expected {needle} in: {err}");
    }
}

#[test]
fn rejects_verdict_template_that_is_not_json() {
    let err = err_of(&GUARDED.replace(
        r#"verdict_template = '{"decision":"block","reason":"{reason}"}'"#,
        "verdict_template = 'block it'",
    ));
    assert!(err.contains("guard.verdict_template"), "{err}");
    assert!(err.contains("JSON"), "{err}");
}

#[test]
fn rejects_verdict_template_without_reason_placeholder() {
    let err = err_of(&GUARDED.replace(
        r#"verdict_template = '{"decision":"block","reason":"{reason}"}'"#,
        r#"verdict_template = '{"decision":"block"}'"#,
    ));
    assert!(err.contains("guard.verdict_template"), "{err}");
    assert!(err.contains("{reason}"), "{err}");
}

#[test]
fn rejects_command_template_missing_exe_or_marker() {
    for (mutated, needle) in [
        (
            r#"command_template = 'eval-magic guard-hook "{marker}"'"#,
            "{exe}",
        ),
        (
            r#"command_template = '"{exe}" guard-hook --harness demo'"#,
            "{marker}",
        ),
    ] {
        let err = err_of(&GUARDED.replace(
            r#"command_template = '"{exe}" guard-hook --harness demo "{marker}"'"#,
            mutated,
        ));
        assert!(err.contains("guard.command_template"), "{err}");
        assert!(err.contains(needle), "expected {needle} in: {err}");
    }
}

#[test]
fn rejects_hooks_file_that_escapes_the_env() {
    for mutated in [
        "hooks_file = \"/etc/hooks.json\"",
        "hooks_file = \"../hooks.json\"",
        "hooks_file = \"./hooks.json\"",
    ] {
        let err = err_of(&GUARDED.replace("hooks_file = \".demo/hooks.json\"", mutated));
        assert!(err.contains("guard.hooks_file"), "{err}");
        assert!(err.contains("relative"), "{err}");
    }
}

/// A guard wired for the OpenCode plugin engine: the install stages an
/// embedded JS plugin at `plugin_file` instead of merging a hook entry
/// into a hook-config file, so the JSON-hooks fields do not apply.
const PLUGIN_GUARDED: &str = r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[run]
supports_guard = true

[tools]
write = ["edit", "write"]
shell = ["bash"]

[guard]
engine = "opencode-plugin"
plugin_file = ".demo/plugins/eval-guard.js"
verdict_template = '{"decision":"block","reason":"{reason}"}'
armed_message = "guard armed"
"#;

#[test]
fn accepts_the_opencode_plugin_engine_shape() {
    load_descriptor(PLUGIN_GUARDED, "test.toml").expect("the plugin engine shape should load");
}

#[test]
fn accepts_an_explicit_json_hooks_engine() {
    let src = GUARDED.replace(
        "hooks_file = \".demo/hooks.json\"",
        "engine = \"json-hooks\"\nhooks_file = \".demo/hooks.json\"",
    );
    load_descriptor(&src, "test.toml").expect("an explicit json-hooks engine should load");
}

#[test]
fn rejects_the_plugin_engine_without_a_plugin_file() {
    let err =
        err_of(&PLUGIN_GUARDED.replace("plugin_file = \".demo/plugins/eval-guard.js\"\n", ""));
    assert!(err.contains("plugin_file"), "{err}");
}

#[test]
fn rejects_a_plugin_file_that_escapes_the_env() {
    for mutated in [
        "plugin_file = \"/etc/eval-guard.js\"",
        "plugin_file = \"../eval-guard.js\"",
        "plugin_file = \"./eval-guard.js\"",
    ] {
        let err = err_of(
            &PLUGIN_GUARDED.replace("plugin_file = \".demo/plugins/eval-guard.js\"", mutated),
        );
        assert!(err.contains("guard.plugin_file"), "{err}");
        assert!(err.contains("relative"), "{err}");
    }
}

#[test]
fn rejects_plugin_engine_with_json_hooks_fields() {
    for (field_line, needle) in [
        ("hooks_file = \".demo/hooks.json\"", "hooks_file"),
        ("matcher = \"write|edit|bash\"", "matcher"),
        (
            "command_template = '\"{exe}\" guard \"{marker}\"'",
            "command_template",
        ),
        ("hook_entry = '{\"matcher\":\"{matcher}\"}'", "hook_entry"),
    ] {
        let src = PLUGIN_GUARDED.replace(
            "engine = \"opencode-plugin\"",
            &format!("engine = \"opencode-plugin\"\n{field_line}"),
        );
        let err = err_of(&src);
        assert!(err.contains(needle), "expected {needle} in: {err}");
        assert!(err.contains("opencode-plugin"), "{err}");
    }
}

#[test]
fn rejects_json_hooks_engine_with_a_plugin_file() {
    // The default engine (no `engine` key) is json-hooks.
    let src = GUARDED.replace(
        "hooks_file = \".demo/hooks.json\"",
        "hooks_file = \".demo/hooks.json\"\nplugin_file = \".demo/plugins/eval-guard.js\"",
    );
    let err = err_of(&src);
    assert!(err.contains("plugin_file"), "{err}");
    assert!(err.contains("json-hooks"), "{err}");
}
