use std::path::Path;

use serde_json::json;

use super::detect_stray_writes;
use crate::adapters::all_tool_vocabulary;
use crate::core::ToolInvocation;
use crate::sandbox::decide::{GuardMarker, decide_with_cwd};

const ALLOWED_ROOT: &str = "/work/iteration-1/env-g1-with_skill";

struct Case {
    command: &'static str,
    cwd: &'static str,
    allow: bool,
}

fn invocation(name: &str, command: &str) -> ToolInvocation {
    ToolInvocation {
        name: name.to_string(),
        args: Some(json!({"command": command})),
        result: None,
        ordinal: 0,
    }
}

#[test]
fn realistic_development_commands_match_between_guard_and_stray_write_audit() {
    let allowed = [
        "npm install",
        "npm --prefix ./web install",
        "npm --prefix './web app' install",
        "npm --global --prefix ./tools install left-pad",
        "npm install global",
        "npm install --location=project left-pad",
        "npm install -- --global",
        "npm install -- --prefix /outside/package-name",
        "pnpm install",
        "pnpm --dir ./web add left-pad",
        "yarn install",
        "yarn --cwd ./web add left-pad",
        "bun install",
        "bun --cwd ./web add left-pad",
        "pip install -r requirements.txt",
        "python -m pip install -e .",
        "pip install --target .venv/lib left-pad",
        "pip install --target '.venv/site packages' left-pad",
        "cargo build",
        "cargo test",
        "CARGO_TARGET_DIR=target cargo test",
        "cargo test -- --target-dir /outside/fixture",
        "npm test",
        "pytest",
        "sed -i 's/old/new/' src/lib.rs",
        "sed -i.bak 's/old/new/' src/lib.rs",
        "sed --in-place=.bak -e 's/old/new/' src/lib.rs",
        "mkdir -p .claude/skills/local-skill",
        "touch skills/local-skill/SKILL.md",
    ]
    .map(|command| Case {
        command,
        cwd: ALLOWED_ROOT,
        allow: true,
    });
    let denied = [
        ("npm install", "/outside"),
        ("pip install left-pad", "/outside"),
        ("cargo build", "/outside"),
        ("cargo test", "/outside"),
        ("sed -i 's/old/new/' src/lib.rs", "/outside"),
        ("npm install --prefix /outside/project", ALLOWED_ROOT),
        ("npm --prefix=\"$PROJECT\" install", ALLOWED_ROOT),
        ("npm install --prefix", ALLOWED_ROOT),
        ("npm install --prefix --global", ALLOWED_ROOT),
        ("npm install --prefix='unterminated", ALLOWED_ROOT),
        ("npm --global install left-pad", ALLOWED_ROOT),
        ("npm install --global=true left-pad", ALLOWED_ROOT),
        ("npm install --location=global left-pad", ALLOWED_ROOT),
        ("npm install --location global left-pad", ALLOWED_ROOT),
        (
            "npm install --location=\"$LOCATION\" left-pad",
            ALLOWED_ROOT,
        ),
        ("pnpm --dir /outside/project install", ALLOWED_ROOT),
        ("pnpm -C \"$PROJECT\" install", ALLOWED_ROOT),
        ("pnpm --global add left-pad", ALLOWED_ROOT),
        ("yarn --cwd /outside/project install", ALLOWED_ROOT),
        ("yarn global add left-pad", ALLOWED_ROOT),
        ("bun --cwd=/outside/project install", ALLOWED_ROOT),
        ("bun install --global left-pad", ALLOWED_ROOT),
        ("pip install --target /outside/site left-pad", ALLOWED_ROOT),
        ("pip install --target", ALLOWED_ROOT),
        (
            "pip install --prefix=/outside/prefix left-pad",
            ALLOWED_ROOT,
        ),
        ("pip install --root /outside/root left-pad", ALLOWED_ROOT),
        ("pip install --src /outside/src -e example", ALLOWED_ROOT),
        ("python -m pip install --user left-pad", ALLOWED_ROOT),
        ("cargo -C /outside/project build", ALLOWED_ROOT),
        ("cargo build --target-dir /outside/target", ALLOWED_ROOT),
        ("cargo build --target-dir", ALLOWED_ROOT),
        ("cargo build --target-dir --release", ALLOWED_ROOT),
        ("CARGO_TARGET_DIR=/outside/target cargo test", ALLOWED_ROOT),
        (
            "CARGO_BUILD_TARGET_DIR=\"$TARGET\" cargo build",
            ALLOWED_ROOT,
        ),
        ("sed -i 's/old/new/' /outside/src/lib.rs", ALLOWED_ROOT),
        ("sed -i 's/old/new/' \"$FILE\"", ALLOWED_ROOT),
        ("printf done > /outside/result.txt", ALLOWED_ROOT),
        ("git push origin main", ALLOWED_ROOT),
    ]
    .map(|(command, cwd)| Case {
        command,
        cwd,
        allow: false,
    });
    let marker = GuardMarker {
        active: Some(true),
        allowed_roots: Some(vec![ALLOWED_ROOT.to_string()]),
        expires_at: None,
        denial_log_path: None,
    };

    for case in allowed.into_iter().chain(denied) {
        for tool in &all_tool_vocabulary().shell_tools {
            let evaluation = decide_with_cwd(
                tool,
                &json!({"command": case.command}),
                Some(&marker),
                0,
                Path::new(case.cwd),
            );
            let findings = detect_stray_writes(
                &[invocation(tool, case.command)],
                ALLOWED_ROOT,
                Path::new(case.cwd),
            );

            assert_eq!(
                evaluation.decision.allow, case.allow,
                "guard mismatch for {tool}: {}",
                case.command
            );
            assert_eq!(
                findings.warnings.is_empty(),
                case.allow,
                "stray-write mismatch for {tool}: {}",
                case.command
            );
            if let Some(finding) = findings.warnings.first() {
                assert!(
                    evaluation
                        .decision
                        .reason
                        .as_deref()
                        .is_some_and(|reason| reason.contains(&finding.reason)),
                    "guard and audit reasons diverged for {tool}: {}",
                    case.command
                );
            }
        }
    }
}
