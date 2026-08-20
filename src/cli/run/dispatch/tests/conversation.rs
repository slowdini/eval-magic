use super::*;

/// The manifest names one dispatch command whatever the plan holds: scripted
/// and one-shot tasks are both runner-driven, so nothing branches on the mix.
#[test]
fn manifest_names_one_dispatch_command_for_scripted_and_one_shot_alike() {
    let turns = vec![ScriptedTurn {
        prompt: "Use US timezones.".into(),
        deliver_when: crate::core::DeliverWhen::AgentAsks,
        agent_response_matches: None,
    }];
    let scripted = build_dispatch_task(&DispatchTaskOpts {
        turns: Some(&turns),
        ..base_opts()
    })
    .unwrap();
    let one_shot = build_dispatch_task(&base_opts()).unwrap();

    let manifest = |tasks: &[DispatchTask]| {
        build_manifest(
            "foo",
            Mode::NewSkill,
            None,
            1,
            "2026-01-01T00:00:00Z",
            tasks,
            ManifestContext {
                harness: Harness::resolve("codex").unwrap(),
                guard: false,
                agent_model: None,
                agent_env: &Default::default(),
            },
        )
    };
    for tasks in [
        vec![scripted.clone()],
        vec![one_shot.clone()],
        vec![scripted, one_shot],
    ] {
        let rendered = manifest(&tasks);
        assert_eq!(
            rendered.matches("eval-magic dispatch --iteration").count(),
            1,
            "one command, whatever the plan holds: {rendered}"
        );
        assert!(
            !rendered.contains("dispatch-task"),
            "the per-task command is gone: {rendered}"
        );
        assert!(
            !rendered.contains("select(.turns == null)"),
            "nothing filters scripted tasks out of a recipe any more: {rendered}"
        );
        assert!(rendered.contains("conversation.json"), "{rendered}");
    }
}
