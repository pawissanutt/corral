//! Startup-failure state transitions only; no successor command is submitted.
use super::*;
use corral_core::agent::AgentRole;

fn prepare_read_only(
    fx: &ts::Fx,
    source: &ContainerRef,
    successor: ContainerRef,
    subject: continuation::SubjectRevision,
    role: AgentRole,
) -> continuation::ContinuationIntent {
    quota(fx, source);
    policy::ensure_root(&fx.ctx, source, "pi", &subject).unwrap();
    let state = ts::state(fx, source);
    let binding = state
        .current_generation()
        .unwrap()
        .connection
        .as_ref()
        .unwrap();
    let target = policy::target(&fx.ctx, "pi", role, binding).unwrap();
    continuation::prepare(
        &fx.ctx,
        continuation::Transfer {
            source: source.clone(),
            successor,
            target: target.binding,
            context: ContextSource::FreshReadOnly {
                reason: "no committed draft".into(),
            },
            subject,
            reason: "confirmed quota".into(),
        },
    )
    .unwrap()
}

fn assert_paused(fx: &ts::Fx, intent: &continuation::ContinuationIntent) {
    let paused = continuation::intent_for(&fx.ctx.store, &intent.successor.container, 1)
        .unwrap()
        .unwrap();
    assert_eq!(paused.phase, IntentPhase::Paused);
    assert_eq!(
        paused.pause,
        Some(continuation::PauseReason::SuccessorAmbiguous)
    );
    assert!(
        continuation::require_launch(
            &fx.ctx.store,
            &intent.successor.container,
            &intent.successor.binding,
        )
        .is_err()
    );
    assert_eq!(
        ts::state(fx, &intent.successor.container).binding.health,
        corral_core::agent::HostHealth::Fenced
    );
}

fn unbound_planner() -> (ts::Fx, String, continuation::ContinuationIntent) {
    let (fx, id, _, source) = fixture();
    let task = fx.ctx.store.get(&id).unwrap();
    let successor = ContainerRef::Planner {
        task: id.clone(),
        run: "D2".into(),
    };
    let intent = prepare_read_only(
        &fx,
        &source,
        successor.clone(),
        continuation::SubjectRevision {
            task: Some(id.clone()),
            spec_revision: Some(task.plan_runs[0].spec_revision.clone()),
            target: None,
        },
        AgentRole::Planner,
    );
    let config = &fx.ctx.store.config;
    let resolved = config
        .resolve_candidate("pi", config.execution_candidate("pi", "api").unwrap())
        .unwrap();
    fx.ctx
        .store
        .update(&id, |task| {
            let mut next = task.plan_runs[0].clone();
            next.id = "D2".into();
            next.agent_host = None;
            next.execution = Some(corral_core::ExecutionSnapshot::new(
                config,
                resolved.clone(),
            ));
            task.plan_runs[0].state = PlanRunState::Cancelled;
            task.plan_runs.push(next);
            Ok(())
        })
        .unwrap();
    ts::bind(
        &fx,
        &BindTarget::Planner {
            task: id.clone(),
            run: "D2".into(),
        },
        "w1:p1",
        "unbound",
    );
    (fx, id, intent)
}

#[test]
fn planner_failed_first_bind_keeps_paused_intent_instead_of_closing_it() {
    let (fx, id, intent) = unbound_planner();
    let current = fx.ctx.store.get(&id).unwrap();
    for _ in 0..2 {
        assert!(
            crate::plan_lifecycle::fail(
                &fx.ctx,
                &current,
                "missing API credential",
                corral_core::clock::now(),
            )
            .unwrap()
        );
        assert_paused(&fx, &intent);
    }
    let saved = fx.ctx.store.get(&id).unwrap();
    assert_eq!(saved.status, Status::NeedsMe);
    assert_eq!(saved.plan_runs[1].state, PlanRunState::Planning);
}

#[test]
fn paused_planner_failed_start_completes_task_hold_once() {
    let (fx, id, intent) = unbound_planner();
    let successor = &intent.successor.container;
    continuation::pause(
        &fx.ctx,
        successor,
        continuation::PauseReason::SuccessorAmbiguous,
        "interrupted before task hold",
    )
    .unwrap();
    assert_paused(&fx, &intent);
    assert_eq!(fx.ctx.store.head(&id).unwrap().status, Status::Running);
    let before = fx.ctx.store.tasks_stamp().unwrap();

    assert!(continuation::failed_start(&fx.ctx, successor, 1).unwrap());
    let held = fx.ctx.store.get(&id).unwrap();
    assert_eq!(held.status, Status::NeedsMe);
    assert_eq!(held.plan_runs[1].state, PlanRunState::Planning);
    let after = fx.ctx.store.tasks_stamp().unwrap();
    assert_ne!(after, before);

    assert!(continuation::failed_start(&fx.ctx, successor, 1).unwrap());
    assert_eq!(fx.ctx.store.tasks_stamp().unwrap(), after);
}

#[test]
fn paused_planner_retry_admission_completes_task_hold_once() {
    for source_retry in [false, true] {
        let (fx, id, intent) = unbound_planner();
        continuation::pause(
            &fx.ctx,
            &intent.successor.container,
            continuation::PauseReason::SuccessorAmbiguous,
            "interrupted before task hold",
        )
        .unwrap();
        let mut expected = fx.ctx.store.get(&id).unwrap();
        assert_eq!(expected.status, Status::Running);
        let before = fx.ctx.store.tasks_stamp().unwrap();
        let recover = || {
            if source_retry {
                crate::plan_dispatch::continuation::already_prepared(
                    &fx.ctx,
                    &intent.source.container,
                )
            } else {
                crate::plan_dispatch::continuation::resume_allowed(
                    &fx.ctx,
                    &intent.successor.container,
                    true,
                )
                .map(|allowed| !allowed)
            }
        };
        assert!(recover().unwrap());
        let held = fx.ctx.store.get(&id).unwrap();
        expected.status = Status::NeedsMe;
        expected.updated = held.updated;
        expected.status_since = Some(held.updated);
        assert_eq!(held, expected);
        let after = fx.ctx.store.tasks_stamp().unwrap();
        assert_ne!(after, before);
        assert!(recover().unwrap());
        assert_eq!(fx.ctx.store.tasks_stamp().unwrap(), after);
    }
}

#[test]
fn plan_reviewer_failed_first_bind_pauses_without_inventing_a_verdict() {
    let (mut fx, id, _, _) = fixture();
    let path = fx._tmp.path().join("submitted.md");
    std::fs::write(&path, PLAN).unwrap();
    let plan = crate::plan_submit(&fx.ctx, &id, &path).unwrap();
    let digest =
        crate::plan::policy_digest(&fx.ctx.store.config, &fx.ctx.store.get(&id).unwrap()).unwrap();
    fx.ctx
        .store
        .update(&id, |task| {
            task.plans[0].reviews.push(serde_json::from_value(json!({
                "id":"PR1", "reviewer":"pi", "policy_digest":digest,
                "at":corral_core::clock::now(), "delivery":"inbox",
                "herdr":{"workspace_id":"w1", "pane_id":"w1:p1", "socket":fx.ctx.herdr.socket_path()?}
            }))?);
            task.plan_runs[0].state = PlanRunState::Reviewing;
            Ok(())
        })
        .unwrap();
    let source = ContainerRef::PlanReviewer {
        task: id.clone(),
        plan: plan.id.clone(),
        review: "PR1".into(),
    };
    let old = ts::bind(
        &fx,
        &BindTarget::PlanReviewer {
            task: id.clone(),
            plan: plan.id.clone(),
            review: "PR1".into(),
        },
        "w1:p1",
        "source",
    );
    ts::committed(&ts::hello(&fx, &old));
    let successor = ContainerRef::PlanReviewer {
        task: id.clone(),
        plan: plan.id.clone(),
        review: "PR2".into(),
    };
    let intent = prepare_read_only(
        &fx,
        &source,
        successor.clone(),
        continuation::SubjectRevision {
            task: Some(id.clone()),
            spec_revision: Some(plan.spec_revision.clone()),
            target: Some(plan.id.clone()),
        },
        AgentRole::PlanReviewer,
    );
    fx.ctx
        .store
        .update(&id, |task| {
            let mut next = task.plans[0].reviews[0].clone();
            next.id = "PR2".into();
            next.agent_host = None;
            task.plans[0].reviews[0].cancelled = true;
            task.plans[0].reviews.push(next);
            Ok(())
        })
        .unwrap();
    ts::bind(
        &fx,
        &BindTarget::PlanReviewer {
            task: id.clone(),
            plan: plan.id.clone(),
            review: "PR2".into(),
        },
        "w1:p1",
        "unbound",
    );
    // The API-only preparation persists this scalar before the process is submitted.
    continuation::with_state(&fx.ctx, &successor, |state| {
        state.paid_episode = Some(intent.link.clone());
        Ok(())
    })
    .unwrap();
    for profile in fx.ctx.store.config.profiles.values_mut() {
        profile
            .metadata
            .as_mut()
            .unwrap()
            .fallback
            .as_mut()
            .unwrap()
            .paid_roles
            .clear();
    }
    let receipt = crate::launch_exit::receipt(&fx.ctx, &id, &format!("{}-PR2", plan.id)).unwrap();
    crate::launch_exit::record_missing(&receipt).unwrap();
    let panes = std::collections::HashMap::new();
    for _ in 0..2 {
        let task = fx.ctx.store.head(&id).unwrap().to_shell_task().unwrap();
        crate::launch_exit::plan_review(
            &fx.ctx,
            &task,
            crate::pane_liveness::Snapshot::injected(&panes),
            corral_core::clock::now(),
        )
        .unwrap();
        assert_paused(&fx, &intent);
    }
    let saved = fx.ctx.store.get(&id).unwrap();
    assert_eq!(saved.status, Status::NeedsMe);
    assert!(saved.plans[0].reviews[1].verdict.is_none());
    assert_eq!(saved.plan_runs[0].state, PlanRunState::Reviewing);
}
