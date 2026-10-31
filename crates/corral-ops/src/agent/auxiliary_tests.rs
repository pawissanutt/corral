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
