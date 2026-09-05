//! Plugin-shadow provenance and aggregate validity warnings.

use super::*;

/// `aggregate`: plugin-shadow findings surface as validity_warnings.
#[test]
fn aggregate_surfaces_plugin_shadow_findings() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    let conditions_path = iteration_dir.join("conditions.json");
    let mut conditions: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&conditions_path).unwrap()).unwrap();
    conditions.as_object_mut().unwrap().remove("harness");
    fs::write(
        &conditions_path,
        serde_json::to_string(&conditions).unwrap(),
    )
    .unwrap();
    for cond in ["with_skill", "without_skill"] {
        write_grading(&iteration_dir, cond, 1.0);
        write_timing(
            &iteration_dir,
            cond,
            json!({"total_tokens": 100, "duration_ms": 1}),
        );
    }
    fs::write(
        iteration_dir.join("plugin-shadow.json"),
        serde_json::to_string(&json!({
            "config_dir": "/home/u/.claude",
            "shadowed": [{"kind": "plugin", "plugin": "slow-powers@slowdini", "skill_name": "mr-review",
                "path": "/home/u/.claude/plugins/cache/slowdini/slow-powers/skills/mr-review"}],
        }))
        .unwrap(),
    )
    .unwrap();

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    let warns = b["validity_warnings"].as_array().unwrap();
    // The legacy warning is user-facing output, so its remediation guidance is
    // self-contained — it must not point at a repository-only development doc a
    // binary-only user cannot open.
    assert!(warns.iter().any(|w| {
        let s = w.as_str().unwrap();
        s.contains("mr-review")
            && s.to_lowercase().contains("contaminat")
            && s.contains("claude -p")
            && s.contains("--setting-sources project,local")
            && s.contains("enabledPlugins")
            && s.contains("CLAUDE_CONFIG_DIR")
    }));
    assert!(warns.iter().all(|w| !w.as_str().unwrap().contains("docs/")));
}

/// `aggregate`: v2 findings carry their own source-specific remediation.
#[test]
fn aggregate_uses_codex_shadow_remediation() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    let mut conditions: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("conditions.json")).unwrap())
            .unwrap();
    conditions["harness"] = json!("codex");
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&conditions).unwrap(),
    )
    .unwrap();
    for cond in ["with_skill", "without_skill"] {
        write_grading(&iteration_dir, cond, 1.0);
        write_timing(
            &iteration_dir,
            cond,
            json!({"total_tokens": 100, "duration_ms": 1}),
        );
    }
    fs::write(
        iteration_dir.join("plugin-shadow.json"),
        serde_json::to_string(&json!({
            "schema_version": 2,
            "config_dir": "/home/u/.codex",
            "findings": [{
                "skill_name": "mr-review",
                "role": "subject",
                "severity": "comparison-invalid",
                "sources": [{
                    "kind": "plugin",
                    "origin": "live",
                    "skill_name": "mr-review",
                    "runtime_id": "mr-review",
                    "plugin": "slow-powers@slowdini",
                    "discovery_path": "/home/u/.codex/plugins/mr-review",
                    "root": {
                        "scope": "global",
                        "namespace": "plugin",
                        "plugin": "slow-powers@slowdini",
                        "path": "/home/u/.codex/plugins",
                        "relation": "native"
                    },
                    "remediation": "Add '--disable plugins' to every Codex dispatch."
                }]
            }],
        }))
        .unwrap(),
    )
    .unwrap();

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    let warns = b["validity_warnings"].as_array().unwrap();
    assert!(warns.iter().any(|w| {
        let s = w.as_str().unwrap();
        s.contains("mr-review")
            && s.contains("--disable plugins")
            && s.contains("comparison invalid")
    }));
}

/// `aggregate`: v2 cross-harness findings retain exact remediation.
#[test]
fn aggregate_uses_opencode_shadow_remediation() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    let mut conditions: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(iteration_dir.join("conditions.json")).unwrap())
            .unwrap();
    conditions["harness"] = json!("opencode");
    fs::write(
        iteration_dir.join("conditions.json"),
        serde_json::to_string(&conditions).unwrap(),
    )
    .unwrap();
    for cond in ["with_skill", "without_skill"] {
        write_grading(&iteration_dir, cond, 1.0);
        write_timing(
            &iteration_dir,
            cond,
            json!({"total_tokens": 100, "duration_ms": 1}),
        );
    }
    fs::write(
        iteration_dir.join("plugin-shadow.json"),
        serde_json::to_string(&json!({
            "schema_version": 2,
            "config_dir": "/home/u/.config/opencode",
            "findings": [{
                "skill_name": "mr-review",
                "role": "subject",
                "severity": "comparison-invalid",
                "sources": [{
                    "kind": "skill",
                    "origin": "live",
                    "skill_name": "mr-review",
                    "runtime_id": "mr-review",
                    "discovery_path": "/home/u/.claude/skills/mr-review",
                    "root": {
                        "scope": "global",
                        "namespace": "claude",
                        "path": "/home/u/.claude/skills",
                        "relation": "cross-harness"
                    },
                    "remediation":
                        "Set OPENCODE_DISABLE_CLAUDE_CODE_SKILLS=1 for every dispatch."
                }]
            }],
        }))
        .unwrap(),
    )
    .unwrap();

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    let warns = b["validity_warnings"].as_array().unwrap();
    assert!(warns.iter().any(|w| {
        let s = w.as_str().unwrap();
        s.contains("mr-review")
            && s.contains("OPENCODE_DISABLE_CLAUDE_CODE_SKILLS")
            && s.contains("cross-harness")
    }));
}

/// `aggregate`: a frozen descriptor assertion downgrades every harness's
/// shadow findings to provenance while retaining the report artifact.
#[test]
fn aggregate_suppresses_declared_isolated_shadows_for_every_harness() {
    use serde_json::json;

    for harness in ["claude-code", "codex", "opencode"] {
        let (_tmp, root) = canonical_root();
        let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
        new_skill_conditions(&iteration_dir, &skill_md);
        let conditions_path = iteration_dir.join("conditions.json");
        let mut conditions: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&conditions_path).unwrap()).unwrap();
        conditions["harness"] = json!(harness);
        fs::write(
            &conditions_path,
            serde_json::to_string(&conditions).unwrap(),
        )
        .unwrap();
        for cond in ["with_skill", "without_skill"] {
            write_grading(&iteration_dir, cond, 1.0);
            write_timing(
                &iteration_dir,
                cond,
                json!({"total_tokens": 100, "duration_ms": 1}),
            );
        }
        fs::write(
            iteration_dir.join("plugin-shadow.json"),
            serde_json::to_string(&json!({
                "config_dir": "/home/u/.config",
                "shadowed": [
                    {"kind": "plugin", "plugin": "slow-powers@slowdini",
                        "skill_name": "mr-review", "path": "/plugins/mr-review"},
                    {"kind": "global-skill", "skill_name": "mr-review",
                        "path": "/skills/mr-review"}
                ],
                "isolates_live_sources": true,
            }))
            .unwrap(),
        )
        .unwrap();

        agg_cmd(&cwd, &skill_dir).assert().success();

        let warnings = read_benchmark(&iteration_dir)["validity_warnings"]
            .as_array()
            .unwrap()
            .clone();
        assert!(
            warnings.iter().all(|warning| {
                !warning
                    .as_str()
                    .is_some_and(|text| text.contains("mr-review"))
            }),
            "{harness} retained a declared-isolated shadow warning: {warnings:?}"
        );
        assert!(
            iteration_dir.join("plugin-shadow.json").exists(),
            "{harness} retains the auditable preflight artifact"
        );
    }
}

#[test]
fn aggregate_keeps_codebase_shadow_warnings_when_operator_sources_are_isolated() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    for cond in ["with_skill", "without_skill"] {
        write_grading(&iteration_dir, cond, 1.0);
        write_timing(
            &iteration_dir,
            cond,
            json!({"total_tokens": 100, "duration_ms": 1}),
        );
    }
    fs::write(
        iteration_dir.join("plugin-shadow.json"),
        serde_json::to_string(&json!({
            "schema_version": 3,
            "config_dir": "/home/u/.claude",
            "isolates_live_sources": true,
            "findings": [{
                "class": "codebase-sourced",
                "skill_name": "mr-review",
                "role": "subject",
                "severity": "comparison-invalid",
                "sources": [{
                    "kind": "skill",
                    "origin": "live",
                    "skill_name": "mr-review",
                    "runtime_id": "mr-review",
                    "discovery_path": "/repo/.claude/skills/mr-review",
                    "root": {
                        "scope": "project",
                        "namespace": "claude",
                        "path": "/repo/.claude/skills",
                        "relation": "native"
                    },
                    "remediation": "Set `codebase.exclude_skill_sources = true` for this eval."
                }]
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    agg_cmd(&cwd, &skill_dir).assert().success();

    let warnings = read_benchmark(&iteration_dir)["validity_warnings"]
        .as_array()
        .unwrap()
        .clone();
    assert!(warnings.iter().any(|warning| {
        warning.as_str().is_some_and(|text| {
            text.contains("mr-review") && text.contains("codebase.exclude_skill_sources")
        })
    }));
}

/// `benchmark.json` is the artifact a published comparison is read from, so the
/// tree each condition ran against has to survive the aggregation step rather
/// than stopping at `conditions.json`.
#[test]
fn aggregate_echoes_the_resolved_skill_source_into_the_benchmark() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    let conditions_path = iteration_dir.join("conditions.json");
    let mut conditions: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&conditions_path).unwrap()).unwrap();
    conditions.as_object_mut().unwrap().insert(
        "skill_source".to_string(),
        json!({
            "kind": "path",
            "source": "/work/skills/mr-review",
            "resolved_path": "/work/skills/mr-review",
            "revision": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
            "branch": "main",
            "host_local": true,
            "dirty": true,
            "siblings": ["helper-skill"]
        }),
    );
    fs::write(
        &conditions_path,
        serde_json::to_string(&conditions).unwrap(),
    )
    .unwrap();
    for cond in ["with_skill", "without_skill"] {
        write_grading(&iteration_dir, cond, 1.0);
        write_timing(
            &iteration_dir,
            cond,
            json!({"total_tokens": 100, "duration_ms": 1}),
        );
    }

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    assert_eq!(
        b["skill_source"]["revision"],
        "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678"
    );
    assert_eq!(b["skill_source"]["dirty"], true);
    assert_eq!(b["skill_source"]["siblings"][0], "helper-skill");
}

#[test]
fn aggregate_echoes_the_resolved_codebases_into_the_benchmark() {
    use serde_json::json;
    let (_tmp, root) = canonical_root();
    let (skill_dir, skill_md, iteration_dir, cwd) = setup_agg(&root);
    new_skill_conditions(&iteration_dir, &skill_md);
    let conditions_path = iteration_dir.join("conditions.json");
    let mut conditions: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&conditions_path).unwrap()).unwrap();
    conditions.as_object_mut().unwrap().insert(
        "codebases".to_string(),
        json!([{
            "kind": "git",
            "source": "https://example.com/project.git",
            "ref": "v1.4.0",
            "revision": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
            "branch": "v1.4.0",
            "exclude_skill_sources": true,
            "evals": ["e1"]
        }]),
    );
    fs::write(
        &conditions_path,
        serde_json::to_string(&conditions).unwrap(),
    )
    .unwrap();
    for cond in ["with_skill", "without_skill"] {
        write_grading(&iteration_dir, cond, 1.0);
        write_timing(
            &iteration_dir,
            cond,
            json!({"total_tokens": 100, "duration_ms": 1}),
        );
    }

    agg_cmd(&cwd, &skill_dir).assert().success();

    let b = read_benchmark(&iteration_dir);
    assert_eq!(
        b["codebases"][0]["source"],
        "https://example.com/project.git"
    );
    assert_eq!(
        b["codebases"][0]["revision"],
        "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678"
    );
    assert_eq!(b["codebases"][0]["evals"][0], "e1");
    assert_eq!(b["codebases"][0]["exclude_skill_sources"], true);
}
