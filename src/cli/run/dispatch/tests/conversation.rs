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

/// The eval's `plan_mode` reaches every task of that eval, and only those, so
/// a task outside plan mode serializes exactly as it did before the field
/// existed. A plan-then-act task still serializes as `true`, which is what
/// keeps older dispatch.json readers working.
#[test]
fn a_plan_mode_task_carries_the_shape_and_a_plain_task_omits_it() {
    let plan = build_dispatch_task(&DispatchTaskOpts {
        plan_mode: PlanMode::PlanThenAct,
        ..base_opts()
    })
    .unwrap();
    let plan_only = build_dispatch_task(&DispatchTaskOpts {
        plan_mode: PlanMode::PlanOnly,
        ..base_opts()
    })
    .unwrap();
    let plain = build_dispatch_task(&base_opts()).unwrap();
    assert_eq!(plan.plan_mode, PlanMode::PlanThenAct);
    assert_eq!(plan_only.plan_mode, PlanMode::PlanOnly);
    assert_eq!(plain.plan_mode, PlanMode::Off);

    let plan_json = serde_json::to_value(&plan).unwrap();
    assert_eq!(plan_json["plan_mode"], serde_json::Value::Bool(true));
    let plan_only_json = serde_json::to_value(&plan_only).unwrap();
    assert_eq!(plan_only_json["plan_mode"], serde_json::json!("plan_only"));
    let plain_json = serde_json::to_value(&plain).unwrap();
    assert!(plain_json.get("plan_mode").is_none(), "{plain_json}");
}

/// The planning instructions are eval-magic's own, so they read the same on
/// every harness: the agent is told it cannot edit and that its final message
/// must carry the whole plan. That is what makes `outputs/plan.md` recoverable
/// on a harness that writes no plan file of its own.
#[test]
fn a_plan_mode_prompt_asks_for_the_whole_plan_and_drops_the_edit_licence() {
    let plan = build_dispatch_task(&DispatchTaskOpts {
        plan_mode: PlanMode::PlanThenAct,
        ..base_opts()
    })
    .unwrap();
    let prompt = &plan.dispatch_prompt;
    assert!(prompt.contains("planning mode"), "{prompt}");
    assert!(
        prompt.contains("complete plan as your final message"),
        "{prompt}"
    );
    assert!(
        !prompt.contains("you may edit existing files"),
        "a planning round is read-only, so the act-mode licence must not appear: {prompt}"
    );
    assert!(
        !prompt.contains("Keep temporary and scratch files"),
        "scratch guidance contradicts a round that cannot write: {prompt}"
    );
    assert!(
        prompt.contains("Do not write outside the task environment."),
        "the containment rule holds in every mode: {prompt}"
    );
}

/// The bullet a non-plan-mode prompt has always carried is untouched, which is
/// what keeps the dispatch-prompt goldens byte-identical.
#[test]
fn a_plain_prompt_keeps_its_act_mode_instructions() {
    let plain = build_dispatch_task(&base_opts()).unwrap();
    let prompt = &plain.dispatch_prompt;
    assert!(prompt.contains("you may edit existing files"), "{prompt}");
    assert!(!prompt.contains("planning mode"), "{prompt}");
}

/// An eval handed a plan runs in ordinary act mode with the plan spliced into
/// its prompt, ahead of the request. That needs nothing from the harness, so
/// it works on a harness with no plan-mode capability at all.
#[test]
fn a_supplied_plan_is_framed_as_approved_and_precedes_the_request() {
    let task = build_dispatch_task(&DispatchTaskOpts {
        plan_text: Some("1. Add an LRU\n2. Cover eviction\n"),
        ..base_opts()
    })
    .unwrap();
    let prompt = &task.dispatch_prompt;
    assert!(prompt.contains("already written and approved"), "{prompt}");
    assert!(prompt.contains("1. Add an LRU"), "{prompt}");
    assert!(prompt.contains("2. Cover eviction"), "{prompt}");
    assert_eq!(task.plan_mode, PlanMode::Off, "a supplied plan is act mode");
    assert!(
        prompt.contains("you may edit existing files"),
        "the act-mode licence still applies: {prompt}"
    );

    let plan_at = prompt.find("already written and approved").unwrap();
    let request_at = prompt.find("User request:").unwrap();
    assert!(
        plan_at < request_at,
        "the plan is context for the request, so it comes first: {prompt}"
    );
}

/// An eval that supplies no plan carries no plan section, so its prompt is
/// byte-identical to what it was before the field existed.
#[test]
fn a_prompt_without_a_supplied_plan_carries_no_plan_section() {
    let task = build_dispatch_task(&base_opts()).unwrap();
    assert!(
        !task
            .dispatch_prompt
            .contains("already written and approved")
    );
}

/// The plan text itself lives in `dispatch-prompt.txt`; the task records which
/// file it came from, so a reader of `dispatch.json` can trace it back.
#[test]
fn a_supplied_plan_records_its_source_path() {
    let task = build_dispatch_task(&DispatchTaskOpts {
        plan_text: Some("the plan\n"),
        plan_source: Some("plans/add-cache.md"),
        ..base_opts()
    })
    .unwrap();
    assert_eq!(task.plan_source.as_deref(), Some("plans/add-cache.md"));

    let plain = build_dispatch_task(&base_opts()).unwrap();
    assert!(plain.plan_source.is_none());
    let plain_json = serde_json::to_value(&plain).unwrap();
    assert!(plain_json.get("plan_source").is_none(), "{plain_json}");
}
