//! Offline real-operation fixtures: durable auxiliary references, cap, replay and fencing.
use super::test_support::*;
use super::*;
use corral_core::agent::{AgentRole, AuxiliaryOutcome, paths};
use serde_json::{Value, json};

#[test]
fn auxiliary_records_are_bound_capped_terminal_and_receipted() {
    let fx = fx();
    let task = worker_task(&fx, "p1");
    let target = BindTarget::Worker {
        task: task.clone(),
        run: "R1".into(),
    };
    let host = bind(&fx, &target, "p1", "s1");
    committed(&hello(&fx, &host));
    let snapshot = fx
        .ctx
        .store
        .config
        .agent
        .capabilities
        .resolve(AgentRole::Worker, None);
    fx.ctx
        .store
        .update(&task, |task| {
            task.runs[0].agent_host.as_mut().unwrap().generations[0].capabilities =
                Some(snapshot.clone());
            Ok(())
        })
        .unwrap();
    let call = |seq, action: &str, request: &str, outcome: Option<&str>| {
        let mut payload =
            json!({"kind":"auxiliary", "action":action, "request":request, "usage_kind":"explore"});
        if action == "prepare" {
            payload["model"] = json!("openai-codex/gpt-6.1-sol");
        }
        if let Some(outcome) = outcome {
            payload["outcome"] = json!(outcome);
        }
        let input = envelope(&host, "event", &format!("ev-1-{seq}"), payload, false);
        send(&fx, &input, "event")
    };
    for i in 0..4 {
        let result = committed(&call(i + 1, "prepare", &format!("child-{i}"), None));
        assert_eq!(
            result.result["session_file"],
            paths::usage_session_file(&fx.ctx.store, &target.container(), &format!("child-{i}"))
                .to_str()
                .unwrap()
        );
        assert_eq!(result.result["generation"], 1);
    }
    for i in 0..3 {
        committed(&call(i + 5, "started", &format!("child-{i}"), None));
    }
    assert_eq!(
        code(&call(8, "started", "child-3", None)),
        Some("bad_request")
    );
    committed(&call(9, "finish", "child-0", Some("completed")));
    committed(&call(10, "started", "child-3", None));
    committed(&call(11, "finish", "child-3", Some("uncertain")));
    // Exact replay is a receipt, not a second reference or outcome transition.
    assert!(matches!(
        call(11, "finish", "child-3", Some("uncertain")),
        CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(
        code(&call(12, "finish", "child-3", Some("completed"))),
        Some("bad_request")
    );
    let saved = state(&fx, &target.container());
    assert_eq!(saved.usage_sessions.len(), 4);
    assert_eq!(
        saved.usage_sessions[3].outcome,
        Some(AuxiliaryOutcome::Uncertain)
    );
    assert_eq!(
        saved.usage_sessions[1].outcome,
        Some(AuxiliaryOutcome::Running)
    );
    fence_generation(&fx.ctx.store, &target.container(), 1, "test-fence").unwrap();
    assert_ne!(code(&call(13, "prepare", "after-fence", None)), None);
    assert_eq!(state(&fx, &target.container()).usage_sessions.len(), 4);
}

#[test]
fn subscription_search_needs_its_snapshot_and_records_usage_without_oracle() {
    let fx = fx();
    let task = worker_task(&fx, "p1");
    let target = BindTarget::Worker {
        task: task.clone(),
        run: "R1".into(),
    };
    let host = bind(&fx, &target, "p1", "s1");
    committed(&hello(&fx, &host));
    let call = |seq, action: &str, request: &str| {
        let mut payload = json!({"kind":"auxiliary", "action":action, "request":request,
            "usage_kind":"web_search"});
        if action == "prepare" {
            payload["model"] = json!("anthropic/claude-sonnet-5-5");
        }
        if action == "finish" {
            payload["outcome"] = json!("uncertain");
        }
        send(
            &fx,
            &envelope(&host, "event", &format!("ev-1-{seq}"), payload, false),
            "event",
        )
    };
    assert_eq!(
        code(&call(1, "prepare", "disabled")),
        Some("op_not_allowed")
    );
    let config: corral_core::agent_capabilities::AgentCapabilities =
        serde_json::from_value(json!({"worker":{"web":{"search_provider":"subscription"}}}))
            .unwrap();
    let snapshot = config.resolve(AgentRole::Worker, None);
    assert!(!snapshot.enabled("oracle"));
    fx.ctx
        .store
        .update(&task, |task| {
            task.runs[0].agent_host.as_mut().unwrap().generations[0].capabilities =
                Some(snapshot.clone());
            Ok(())
        })
        .unwrap();
    committed(&call(2, "prepare", "search-1"));
    committed(&call(3, "started", "search-1"));
    committed(&call(4, "prepare", "search-2"));
    assert_eq!(code(&call(5, "started", "search-2")), Some("bad_request"));
    committed(&call(6, "finish", "search-1"));
    committed(&call(7, "started", "search-2"));
    let saved = state(&fx, &target.container());
    assert_eq!(
        saved.usage_sessions[0].kind,
        corral_core::agent::UsageSessionKind::WebSearch
    );
    assert_eq!(
        saved.usage_sessions[0].outcome,
        Some(AuxiliaryOutcome::Uncertain)
    );
}

#[test]
fn retired_oracle_rejects_stale_enabled_prepare_but_settles_existing_records() {
    use corral_core::agent::{UsageSession, UsageSessionKind};

    let fx = fx();
    let task = worker_task(&fx, "p1");
    let target = BindTarget::Worker {
        task: task.clone(),
        run: "R1".into(),
    };
    let host = bind(&fx, &target, "p1", "s1");
    committed(&hello(&fx, &host));
    let mut snapshot = fx
        .ctx
        .store
        .config
        .agent
        .capabilities
        .resolve(AgentRole::Worker, None);
    let mut stale_feature = snapshot.features.get("subagents").unwrap().clone();
    stale_feature.enabled = true;
    snapshot.features.insert("oracle".into(), stale_feature);
    assert!(snapshot.enabled("oracle"));
    fx.ctx
        .store
        .update(&task, |task| {
            let state = task.runs[0].agent_host.as_mut().unwrap();
            state.generations[0].capabilities = Some(snapshot.clone());
            for (request, outcome) in [
                ("prepared", AuxiliaryOutcome::Prepared),
                ("running", AuxiliaryOutcome::Running),
                ("cancel", AuxiliaryOutcome::Prepared),
            ] {
                state.usage_sessions.push(UsageSession {
                    generation: 1,
                    kind: UsageSessionKind::Oracle,
                    session_id: request.into(),
                    session_file: paths::usage_session_file(
                        &fx.ctx.store,
                        &target.container(),
                        request,
                    ),
                    model: Some("historical-model".into()),
                    request: Some(request.into()),
                    outcome: Some(outcome),
                });
            }
            Ok(())
        })
        .unwrap();
    let call = |seq, action: &str, request: &str, outcome: Option<&str>| {
        let mut payload = json!({"kind":"auxiliary", "action":action, "request":request,
            "usage_kind":"oracle"});
        if action == "prepare" {
            payload["model"] = json!("model");
        }
        if let Some(outcome) = outcome {
            payload["outcome"] = json!(outcome);
        }
        send(
            &fx,
            &envelope(&host, "event", &format!("ev-1-{seq}"), payload, false),
            "event",
        )
    };
    assert_eq!(
        code(&call(1, "prepare", "new", None)),
        Some("op_not_allowed")
    );
    fx.ctx
        .store
        .update(&task, |task| {
            task.runs[0].agent_host.as_mut().unwrap().generations[0].capabilities = None;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        code(&call(2, "started", "missing", None)),
        Some("bad_request")
    );
    // Another historical invocation is already running; a missing snapshot must not
    // impose a new default parallel limit on work that was previously admitted.
    committed(&call(3, "started", "prepared", None));
    committed(&call(4, "finish", "prepared", Some("completed")));
    committed(&call(5, "finish", "running", Some("uncertain")));
    committed(&call(6, "finish", "cancel", Some("not_started")));
    assert!(matches!(
        call(6, "finish", "cancel", Some("not_started")),
        CallResponse::Receipt { replayed: true, .. }
    ));
    let saved = state(&fx, &target.container());
    assert_eq!(saved.usage_sessions.len(), 3);
    let saved: corral_core::agent::AgentRunState =
        serde_json::from_value(serde_json::to_value(saved).unwrap()).unwrap();
    for (record, outcome) in saved.usage_sessions.iter().zip([
        AuxiliaryOutcome::Completed,
        AuxiliaryOutcome::Uncertain,
        AuxiliaryOutcome::NotStarted,
    ]) {
        assert_eq!(record.kind, UsageSessionKind::Oracle);
        assert_eq!(record.model.as_deref(), Some("historical-model"));
        assert_eq!(record.outcome.as_ref(), Some(&outcome));
    }
}

#[test]
fn auxiliary_prepare_refuses_disabled_feature_and_path_injection() {
    let fx = fx();
    let task = worker_task(&fx, "p1");
    let target = BindTarget::Worker {
        task,
        run: "R1".into(),
    };
    let host = bind(&fx, &target, "p1", "s1");
    committed(&hello(&fx, &host));
    let call = |seq, payload: Value| {
        send(
            &fx,
            &envelope(&host, "event", &format!("ev-1-{seq}"), payload, false),
            "event",
        )
    };
    assert_eq!(
        code(&call(
            1,
            json!({"kind":"auxiliary", "action":"prepare", "request":"oracle", "usage_kind":"oracle", "model":"model"})
        )),
        Some("op_not_allowed")
    );
    assert_eq!(
        code(&call(
            2,
            json!({"kind":"auxiliary", "action":"prepare", "request":"../../escape", "usage_kind":"explore", "model":"model"})
        )),
        Some("bad_request")
    );
    assert!(state(&fx, &target.container()).usage_sessions.is_empty());
}
