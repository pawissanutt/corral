//! Incoming-main native plan-review launch must not silently become a legacy pane command.
use super::*;
use corral_core::agent::{AgentRole, ContainerRef, paths};
use corral_core::inbox::DeliveryMode;

#[test]
fn real_plan_review_service_binds_native_generation_and_bootstrap_before_submit() {
    let (tmp, mut ctx, id) = preview_fixture();
    let assets = tmp.path().join("native-review-assets");
    for (relative, text) in [
        ("package.json".to_owned(), "{\"name\":\"corral-pi\"}"),
        ("bun.lock".into(), "{}"),
        (
            "compat.json".into(),
            "{\"version\":1,\"verdict\":\"GO\",\"bun\":{\"range\":\"1.4.2\"},\"capabilities\":[\"role_enabled\"]}",
        ),
        ("fixture.json".into(), "{\"fixture\":true}"),
        (
            crate::agent::launch::HOST_ENTRY.into(),
            "// inert fixture host",
        ),
        (
            format!(
                "src/roles/{}/index.ts",
                AgentRole::PlanReviewer.module_dir()
            ),
            "export default {};",
        ),
        ("src/delivery/index.ts".into(), "export default {};"),
        ("src/auth/index.ts".into(), "export default {};"),
    ] {
        let path = assets.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    ctx.store.config.agent.assets_dir = Some(assets.clone());
    ctx.store.config.review.reviewer = Some("gpt-review".into());
    let profile = ctx.store.config.profiles.get_mut("gpt-review").unwrap();
    profile.harness = corral_core::CORRAL_PI_HARNESS.into();
    profile.model = Some("gpt-6.1-sol".into());
    profile.vendor = Some("openai".into());
    ctx.store.save_config().unwrap();
    let plan = ctx.store.get(&id).unwrap().plans.last().unwrap().clone();
    let source = exact_review_only_source(&ctx, &id, &plan);
    let source_before = ctx.store.get_assistant_container(&source).unwrap();
    let producer = ctx.store.get(&id).unwrap().plan_runs[0].clone();
    // This fake engine's only pane lives entirely in the disposable fixture.
    std::fs::write(tmp.path().join("engine/planner-live"), "").unwrap();
    let review = crate::plan_review(&ctx, &id, Some("gpt-review")).unwrap();
    assert_eq!(review.delivery, DeliveryMode::Inbox);
    let state = review
        .agent_host
        .as_ref()
        .expect("real native state in the complete return");
    assert_eq!(state.binding.role, AgentRole::PlanReviewer);
    assert_eq!(state.binding.generation, 1);
    assert!(state.current_generation().is_some());
    let container = ContainerRef::PlanReviewer {
        task: id.clone(),
        plan: plan.id.clone(),
        review: review.id.clone(),
    };
    let stored = ctx.store.head(&id).unwrap();
    let actual = stored.agent_generation(&container, 1).unwrap().unwrap();
    assert_eq!(actual.pane, review.herdr.as_ref().unwrap().pane_id);
    let command = std::fs::read_to_string(tmp.path().join("engine/launch-command")).unwrap();
    assert!(command.contains("agent launch --bootstrap"), "{command}");
    assert!(command.contains(&assets.display().to_string()));
    assert!(!command.contains("fixture-planner"));
    assert_eq!(
        ctx.store
            .get(&id)
            .unwrap()
            .plans
            .last()
            .unwrap()
            .reviews
            .last()
            .unwrap(),
        &review
    );
    // Bind the generation the NORMAL service launched, rather than allocating a
    // second one. This is an authenticated protocol fixture, not a real SDK/model.
    let reviewer = reviewer_from_bootstrap(&ctx, &container);
    let fx = crate::agent::test_support::Fx {
        _tmp: tempfile::tempdir().unwrap(),
        ctx: ctx.clone(),
    };
    crate::agent::test_support::committed(&crate::agent::test_support::hello(&fx, &reviewer));
    let request =
        crate::agent::tool_request_id(&reviewer.identity.session_id, "review-only-verdict");
    let notes = "Fixture independent findings: clarify the widget rotation bound.";
    let payload = serde_json::json!({"verdict":"changes","notes":notes});
    let input =
        crate::agent::test_support::envelope(&reviewer, "plan_verdict", &request, payload, false);
    let recorded = crate::agent::test_support::committed(&crate::agent::test_support::send(
        &fx,
        &input,
        "plan_verdict",
    ));
    assert_eq!(recorded.result["outcome"], "recorded");
    let replay = crate::agent::test_support::send(&fx, &input, "plan_verdict");
    assert!(matches!(
        &replay,
        crate::agent::CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(replay.receipt(), Some(&recorded));
    assert!(ctx.store.config.autopilot_enabled());
    for _ in 0..2 {
        let current = ctx.store.get(&id).unwrap();
        crate::plan_pilot::reconcile(
            &ctx,
            &current,
            crate::pane_liveness::Snapshot::injected(&std::collections::HashMap::new()),
            Timestamp::now(),
        )
        .unwrap();
    }
    let stopped = ctx.store.get(&id).unwrap();
    assert_eq!(stopped.plans.len(), 1);
    assert_eq!(stopped.plan_runs.len(), 1);
    assert_eq!(stopped.plan_runs[0].id, producer.id);
    assert_eq!(stopped.plan_runs[0].profile, producer.profile);
    assert_eq!(stopped.plan_runs[0].rounds, producer.rounds);
    assert_eq!(stopped.plans[0].run, plan.run);
    assert_eq!(stopped.plans[0].artifact, plan.artifact);
    assert_eq!(stopped.plans[0].reviews.len(), 1);
    assert_eq!(stopped.plans[0].reviews[0].verdict, Some(Verdict::Changes));
    assert_eq!(stopped.plans[0].reviews[0].notes.as_deref(), Some(notes));
    assert!(
        stopped.plans[0].approval.is_none()
            && stopped.runs.is_empty()
            && stopped.launch.is_none()
            && stopped.landed.is_none()
    );
    assert_eq!(
        ctx.store.get_assistant_container(&source).unwrap(),
        source_before
    );
    assert!(
        crate::operating::require(
            &ctx.store,
            &stopped,
            corral_core::OperatingActionKind::Dispatch
        )
        .is_err()
    );
    eprintln!(
        "REVIEW_ONLY_NATIVE: normal dedicated independent launch + actor-bound Changes receipt/replay; exact plan/spec/producer retained, terminal pilot does not rewrite/replace planner or start code/gate/landing; scripted protocol fixture only"
    );
    let _ = std::fs::remove_dir_all(
        crate::agent::launch::runtime_root().join(paths::store_id(&ctx.store)),
    );
}

fn source_expected(
    c: &corral_core::assistant::AssistantContainer,
) -> crate::assistant::workflow::Expected {
    let w = &c.workflow.as_ref().unwrap().current;
    crate::assistant::workflow::Expected {
        workflow: w.id.clone(),
        state_revision: w.state_revision,
        authority_epoch: w.authority_epoch,
        plan_revision: w.current_plan.as_ref().map(|p| p.revision),
    }
}

fn exact_review_only_source(ctx: &Ctx, id: &str, plan: &corral_core::PlanRecord) -> String {
    use crate::assistant::workflow::{self, handoff as h};
    use corral_core::assistant::{NewOwnerInput, OwnerInputSource};
    use corral_core::assistant_workflow::{DesignStage, WorkflowPhase};
    use corral_core::blob_ref::BlobHash;
    let session = ctx
        .store
        .create_assistant("gpt-sol", Some(id), None, None)
        .unwrap()
        .id;
    // Explicit test provenance; no real native owner/choice consent is claimed.
    ctx.store
        .update_assistant_container(&session, |c| {
            let text = "fixture: review this exact existing plan, no rewrite or implementation";
            let input = c.journal_owner_input(
                &session,
                NewOwnerInput {
                    generation: c.state.binding.generation,
                    turn: None,
                    source: OwnerInputSource::Pane,
                    text: text.into(),
                    sha256: BlobHash::of(text.as_bytes()).to_string(),
                    truncated: false,
                    command: None,
                    answers: None,
                    request: "fixture-review-source".into(),
                },
            );
            workflow::prepare_entry(&ctx.store, &session, c, &input.id)?;
            c.workflow.as_mut().unwrap().current.phase = WorkflowPhase::Planning;
            Ok(())
        })
        .unwrap();
    let bytes = crate::artifact_storage::artifact_read_confined_limited(
        ctx,
        &plan.artifact,
        corral_core::assistant_workflow::DOCUMENT_MAX as u64,
    )
    .unwrap();
    let c = ctx.store.get_assistant_container(&session).unwrap();
    let expected = source_expected(&c);
    workflow::stage(
        &ctx.store,
        &session,
        &c,
        &expected,
        0,
        std::str::from_utf8(&bytes).unwrap(),
    )
    .unwrap();
    ctx.store
        .update_assistant_container(&session, |c| {
            workflow::publish(
                &ctx.store,
                &session,
                c,
                workflow::Publish {
                    expected: expected.clone(),
                    bytes: bytes.len() as u32,
                    digest: BlobHash::of(&bytes),
                    stage: DesignStage::Draft,
                    execution_scope: None,
                    assumptions: None,
                    scope_changes: String::new(),
                },
            )
            .map(drop)
        })
        .unwrap();
    let c = ctx.store.get_assistant_container(&session).unwrap();
    let request=h::Request {project:"app".into(),scope:vec!["widget.rs".into()],goal:h::Goal::PlanReviewOnly,
        context:serde_json::from_value(serde_json::json!({"mode":"task","brief":{
            "goal":"review exact existing plan","constraints":["no author rewrite or implementation"],"decisions":[],"completed":[],
            "remaining":["independent plan review"],"findings":[],"failed_approaches":[],"assumptions":[],"evidence":[],
            "questions":[],"mandatory_blockers":[],"next_action":"one independent verdict"},"history":null})).unwrap()};
    let prepared = h::prepare(
        &ctx.store,
        &session,
        &c,
        &source_expected(&c),
        request,
        None,
    )
    .unwrap();
    let choice = h::transfer::preview(
        ctx,
        prepared.id(),
        "Review exact existing plan",
        Some(id),
        false,
    )
    .unwrap();
    let adopted = h::transfer::commit(
        ctx,
        &choice,
        &choice.digest().unwrap(),
        h::lifecycle::Instruction::Shell {
            text: "fixture owner selected exact review-only target".into(),
            at: corral_core::clock::now(),
        },
    )
    .unwrap();
    assert_eq!(adopted.id, id);
    assert_eq!(adopted.plans.len(), 1);
    assert_eq!(adopted.plans[0].run, plan.run);
    assert_eq!(adopted.plans[0].artifact, plan.artifact);
    session
}

fn reviewer_from_bootstrap(
    ctx: &Ctx,
    container: &ContainerRef,
) -> crate::agent::test_support::Host {
    use crate::agent::{
        launch::{BOOTSTRAP_FILE, Bootstrap, generation_dir},
        test_support as ts,
    };
    let ContainerRef::PlanReviewer { task, plan, review } = container else {
        panic!("reviewer fixture")
    };
    let recipient = corral_core::inbox::Recipient::PlanReview {
        task: task.clone(),
        plan: plan.clone(),
        review: review.clone(),
    };
    let b: Bootstrap = serde_json::from_slice(
        &std::fs::read(
            generation_dir(&ctx.store, &recipient, 1)
                .unwrap()
                .join(BOOTSTRAP_FILE),
        )
        .unwrap(),
    )
    .unwrap();
    ts::Host {
        identity: serde_json::from_value(
            serde_json::json!({"store":b.store,"project":b.project,"role":b.role,
        "task":b.task,"run":b.run,"review":b.review,"callback":b.callback,"assistant":b.assistant,
        "session_id":"fixture-c3-independent-review","generation":b.generation}),
        )
        .unwrap(),
        host_cap: ts::secret("fixture-c3-review-host", b.generation),
        ctl_cap: ts::secret("fixture-c3-review-ctl", b.generation),
        nonce: b.bind_nonce,
        pane: b.pane,
        sessions: b.session_dir,
    }
}
