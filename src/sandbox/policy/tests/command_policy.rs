use super::*;

#[test]
fn allows_configured_package_install_from_an_allowed_cwd() {
    let roots = vec!["/work/env".to_string()];
    let policy = crate::core::GuardPolicyConfig {
        allow_commands: vec!["npm install".to_string()],
        ..crate::core::GuardPolicyConfig::default()
    };

    assert_eq!(
        classify_bash_with_policy(
            "npm install left-pad",
            &roots,
            Path::new("/work/env"),
            &policy,
        ),
        None
    );
}

#[test]
fn allows_only_matching_prefixes_for_a_claimed_tool() {
    let roots = vec!["/work/env".to_string()];
    let policy = crate::core::GuardPolicyConfig {
        profiles: Vec::new(),
        allow_tools: Vec::new(),
        allow_commands: vec!["npm run dev".to_string()],
    };

    assert_eq!(
        classify_bash_with_policy(
            "npm run dev -- --host 127.0.0.1",
            &roots,
            Path::new("/work/env"),
            &policy,
        ),
        None
    );
    for command in [
        "npm install",
        "npm $SUBCOMMAND",
        "npm run dev 'unterminated",
    ] {
        assert_eq!(
            classify_bash_with_policy(command, &roots, Path::new("/work/env"), &policy)
                .map(|denial| denial.reason),
            Some("command not allowed by eval guard policy"),
            "{command}",
        );
    }
    assert_eq!(
        classify_bash_with_policy("cargo metadata", &roots, Path::new("/work/env"), &policy,),
        None
    );
}

#[test]
fn recognized_development_mutations_require_an_allowance() {
    let roots = vec!["/work/env".to_string()];
    let policy = crate::core::GuardPolicyConfig::default();

    for command in [
        "npm install",
        "pip install -r requirements.txt",
        "cargo build",
        "npx next dev",
        "python -m pytest",
        "sed -i 's/old/new/' src/lib.rs",
    ] {
        assert_eq!(
            classify_bash_with_policy(command, &roots, Path::new("/work/env"), &policy)
                .map(|denial| denial.reason),
            Some("command not allowed by eval guard policy"),
            "{command}",
        );
    }
}

#[test]
fn matches_wrapped_commands_and_each_compound_segment() {
    let roots = vec!["/work/env".to_string()];
    let policy = crate::core::GuardPolicyConfig {
        allow_commands: vec!["cargo test".to_string(), "npm run dev".to_string()],
        ..crate::core::GuardPolicyConfig::default()
    };

    for command in [
        "MODE=ci /usr/bin/cargo test --workspace",
        "env MODE=ci cargo test",
        "command cargo test",
        "exec cargo test",
        "exec -a worker cargo test",
        "nice -n 5 cargo test",
        "timeout 30s cargo test",
        "sh -c 'cargo test --workspace'",
    ] {
        assert_eq!(
            classify_bash_with_policy(command, &roots, Path::new("/work/env"), &policy),
            None,
            "{command}",
        );
    }

    for command in [
        "npm run dev && npm install",
        "sh -c 'npm run dev && npm install'",
        "exec -a worker npm install",
    ] {
        assert_eq!(
            classify_bash_with_policy(command, &roots, Path::new("/work/env"), &policy)
                .map(|denial| denial.reason),
            Some("command not allowed by eval guard policy"),
            "{command}",
        );
    }
}

#[test]
fn allow_tools_applies_to_a_shell_wrapper_itself() {
    let roots = vec!["/work/env".to_string()];
    let policy = crate::core::GuardPolicyConfig {
        allow_tools: vec!["sh".to_string()],
        ..crate::core::GuardPolicyConfig::default()
    };

    assert_eq!(
        classify_bash_with_policy(
            "sh -c 'npm install'",
            &roots,
            Path::new("/work/env"),
            &policy,
        ),
        None
    );
}

#[test]
fn shell_wrapper_allowance_cannot_bypass_fixed_containment() {
    let roots = vec!["/work/env".to_string()];
    let policy = crate::core::GuardPolicyConfig {
        allow_tools: vec!["sh".to_string()],
        ..crate::core::GuardPolicyConfig::default()
    };

    assert_eq!(
        classify_bash_with_policy(
            "sh -c 'cargo build --target-dir /outside/target'",
            &roots,
            Path::new("/work/env"),
            &policy,
        )
        .map(|classification| classification.reason),
        Some("cargo build/test output")
    );
}
