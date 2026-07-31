//! Invariant tests, split by the descriptor section each invariant guards.
//! Fixtures used by more than one section live here; section-local ones live
//! beside the tests that use them.

use super::super::load_descriptor;

mod conversation;
mod dispatch;
mod guard;
mod staging;
mod tools;
mod transcript;

/// Load through the full pipeline so each rejection test proves the
/// invariant fires on a descriptor that already passed the schema gate.
fn err_of(toml_src: &str) -> String {
    load_descriptor(toml_src, "test.toml")
        .expect_err("descriptor should be rejected")
        .to_string()
}

const MINIMAL: &str = r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]
"#;

/// Transcript-shape tests need a tool vocabulary so only the shape rule
/// under test can fire.
const TOOLED: &str = r#"
label = "demo"
skills_dir = ".demo/skills"
config_dirs = [".demo"]

[tools]
write = ["file_change"]
shell = ["command_execution"]
"#;
