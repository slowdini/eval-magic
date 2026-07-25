use super::*;

#[test]
fn manifest_routes_scripted_tasks_through_the_conversation_driver() {
    let turns = vec![ScriptedTurn {
        prompt: "Use US timezones.".into(),
        deliver_when: crate::core::DeliverWhen::AgentAsks,
        agent_response_matches: None,
    }];
    let task = build_dispatch_task(&DispatchTaskOpts {
        turns: Some(&turns),
        ..base_opts()
    })
    .unwrap();
    let manifest = build_manifest(
        "foo",
        Mode::NewSkill,
        None,
        1,
        "2026-01-01T00:00:00Z",
        &[task],
        ManifestContext {
            harness: Harness::resolve("codex").unwrap(),
            guard: false,
            agent_model: None,
        },
    );
    assert!(manifest.contains("eval-magic dispatch-task"));
    assert!(manifest.contains("--task-index 0"));
    assert!(manifest.contains("conversation.json"));
    assert!(
        !manifest.contains("codex --ask-for-approval never exec --cd"),
        "all-scripted manifests must not advertise the one-shot command"
    );
}

#[test]
fn mixed_manifest_filters_scripted_tasks_out_of_the_one_shot_recipe() {
    let turns = vec![ScriptedTurn {
        prompt: "Use US timezones.".into(),
        deliver_when: crate::core::DeliverWhen::Always,
        agent_response_matches: None,
    }];
    let scripted = build_dispatch_task(&DispatchTaskOpts {
        turns: Some(&turns),
        ..base_opts()
    })
    .unwrap();
    let one_shot = build_dispatch_task(&base_opts()).unwrap();

    let manifest = build_manifest(
        "foo",
        Mode::NewSkill,
        None,
        1,
        "2026-01-01T00:00:00Z",
        &[scripted, one_shot],
        ManifestContext {
            harness: Harness::resolve("codex").unwrap(),
            guard: false,
            agent_model: None,
        },
    );

    assert!(manifest.contains("eval-magic dispatch-task"));
    assert!(
        manifest.contains(".tasks[] | select(.turns == null)"),
        "one-shot parallel recipe must exclude scripted tasks: {manifest}"
    );
}
