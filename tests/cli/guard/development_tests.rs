use super::*;

#[test]
fn guard_allows_configured_development_tools_from_the_environment() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join(".eval-magic");
    fs::create_dir_all(&workspace).unwrap();
    let marker = write_armed_marker(tmp.path(), &workspace);
    let mut marker_value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&marker).unwrap()).unwrap();
    marker_value["guardPolicy"] = serde_json::json!({
        "allow_tools": ["npm", "pip", "cargo", "sed"]
    });
    fs::write(&marker, serde_json::to_string(&marker_value).unwrap()).unwrap();

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
