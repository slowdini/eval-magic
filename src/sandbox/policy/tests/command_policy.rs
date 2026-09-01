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

#[test]
fn denials_reports_every_denying_layer_containment_first() {
    let roots = vec!["/work/env".to_string()];
    let policy = crate::core::GuardPolicyConfig {
        allow_commands: vec!["cargo build".to_string()],
        ..crate::core::GuardPolicyConfig::default()
    };

    let denials = classify_bash_denials(
        "npm run dev > /tmp/dev-server.log",
        &roots,
        Path::new("/work/env"),
        &policy,
    );

    let reasons: Vec<&str> = denials.iter().map(|denial| denial.reason).collect();
    assert_eq!(
        reasons,
        [
            "output redirection to a file",
            "command not allowed by eval guard policy"
        ]
    );
    assert_eq!(
        denials[0].resolved_targets,
        vec!["/tmp/dev-server.log".to_string()]
    );
}

#[test]
fn denials_reports_a_single_layer_when_only_one_applies() {
    let roots = vec!["/work/env".to_string()];

    // Containment only: echo is unrecognized by the command policy.
    let denials = classify_bash_denials(
        "echo hi > /tmp/out.log",
        &roots,
        Path::new("/work/env"),
        &crate::core::GuardPolicyConfig::default(),
    );
    assert_eq!(denials.len(), 1);
    assert_eq!(denials[0].reason, "output redirection to a file");

    // Policy only: the redirect target is in bounds, but npm is claimed and
    // not allowed.
    let policy = crate::core::GuardPolicyConfig {
        allow_commands: vec!["cargo build".to_string()],
        ..crate::core::GuardPolicyConfig::default()
    };
    let denials = classify_bash_denials(
        "npm run dev > /work/env/dev-server.log",
        &roots,
        Path::new("/work/env"),
        &policy,
    );
    assert_eq!(denials.len(), 1);
    assert_eq!(
        denials[0].reason,
        "command not allowed by eval guard policy"
    );
}

#[test]
fn denials_is_empty_for_an_allowed_command() {
    let roots = vec!["/work/env".to_string()];
    let policy = crate::core::GuardPolicyConfig {
        allow_commands: vec!["cargo build".to_string()],
        ..crate::core::GuardPolicyConfig::default()
    };

    assert!(
        classify_bash_denials("cargo build", &roots, Path::new("/work/env"), &policy).is_empty()
    );
}
