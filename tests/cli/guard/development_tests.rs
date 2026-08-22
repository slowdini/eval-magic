use super::*;

#[test]
fn guard_allows_realistic_development_commands_from_the_environment() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join(".eval-magic");
    fs::create_dir_all(&workspace).unwrap();
    let marker = write_armed_marker(tmp.path(), &workspace);

    for command in [
        "npm install",
        "pip install -r requirements.txt",
        "cargo build",
        "npm test",
        "sed -i 's/old/new/' src/lib.rs",
    ] {
        skill_eval()
            .arg("guard")
            .arg(&marker)
            .write_stdin(
                serde_json::json!({
                    "tool_name": "Bash",
                    "cwd": workspace,
                    "tool_input": { "command": command },
                })
                .to_string(),
            )
            .assert()
            .success()
            .stdout("");
    }
}
