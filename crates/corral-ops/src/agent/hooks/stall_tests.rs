//! Native provider evidence must come from indexed turns, never empty compatibility state.
use super::*;
use crate::agent::bridge::fixture as f;
use corral_core::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth};
use corral_core::work::{self, Counter, HotScope};

#[test]
fn typed_native_failures_classify_without_text_and_old_turns_still_load() {
    use corral_core::availability::{FailureCategory, ProviderFailure};
    let old = json!({"generation":1,"seq":1,"origin":{"kind":"unknown"},"started":"2026-01-01T00:00:00Z",
        "ended":"2026-01-01T00:00:01Z","class":"provider_failed","provider_error":"rate_limit"});
    let legacy: AgentTurn = serde_json::from_value(old.clone()).unwrap();
    assert!(legacy.provider_failure.is_none());
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("provider_failure")
            .is_none()
    );
    assert!(matches!(
        stall(1, Some(&legacy)).unwrap().class,
        AvailabilityClass::Transient { .. }
    ));
    for category in [
        "subscription_quota",
        "transient",
        "auth",
        "billing",
        "invalid_request",
        "unknown",
        "local_budget",
        "network_policy",
    ] {
        let mut value = old.clone();
        let paid = category == "billing";
        let evidence = json!({"version":1,"connection":if paid {"openai-api"} else {"openai-subscription"},
            "category":category,"scope":if category == "subscription_quota" {"account"} else {"unknown"},
            "retry_after_secs":if category == "transient" {Some(120)} else {None}});
        let failure: ProviderFailure = serde_json::from_value(evidence.clone()).unwrap();
        failure.validate().unwrap();
        value["provider_error"] = json!(failure.class());
        value["provider_failure"] = evidence;
        let turn: AgentTurn = serde_json::from_value(value).unwrap();
        match category {
            "subscription_quota" => assert!(matches!(
                stall(1, Some(&turn)).unwrap().class,
                AvailabilityClass::QuotaExhausted { account: true, .. }
            )),
            "transient" => assert_eq!(
                stall(1, Some(&turn)).unwrap().class,
                AvailabilityClass::Transient {
                    retry_after_secs: Some(120)
                }
            ),
            _ => {
                assert!(stall(1, Some(&turn)).is_none());
                assert!(terminal_failure(Some(&turn)).is_some());
            }
        }
        if paid {
            let mut bogus = failure;
            bogus.category = FailureCategory::SubscriptionQuota;
            bogus.scope = corral_core::availability::FailureScope::Account;
            assert!(bogus.validate().is_err());
            assert!(bogus.availability().is_none());
        }
    }
}

#[test]
fn native_failure_event_binds_connection_persists_reset_and_replays_once() {
    use crate::agent::{self, test_support as ts};
    let fx = ts::fx();
    let id = ts::worker_task(&fx, "p1");
    let target = agent::BindTarget::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    let host = ts::bind(&fx, &target, "p1", "failure-session");
    ts::committed(&ts::hello(&fx, &host));
    let send = |seq, payload| {
        let input = ts::envelope(
            &host,
            "event",
            &agent::event_request_id(1, seq),
            payload,
            false,
        );
        ts::send(&fx, &input, "event")
    };
    ts::committed(&send(
        1,
        json!({"kind":"turn_started","turn":1,"provider":"openai-codex","model":"fixture-model"}),
    ));
    let failure = json!({"version":1,"connection":"openai-subscription","category":"subscription_quota","scope":"account",
        "reset_at":"2027-01-15T08:10:00Z","request_ref":"a".repeat(64)});
    let mut wrong = failure.clone();
    wrong["connection"] = json!("anthropic-subscription");
    assert_eq!(
        ts::code(&send(
            2,
            json!({"kind":"provider_error","turn":1,"class":"quota","failure":wrong})
        )),
        Some("bad_request")
    );
    let mut raw = failure.clone();
    raw["raw_body"] = json!("secret");
    assert_eq!(
        ts::code(&send(
            3,
            json!({"kind":"provider_error","turn":1,"class":"quota","failure":raw})
        )),
        Some("bad_request")
    );
    let payload = json!({"kind":"provider_error","turn":1,"class":"quota","failure":failure});
    let receipt = ts::committed(&send(4, payload.clone()));
    assert_eq!(receipt.result["failure"], failure);
    assert!(matches!(
        send(4, payload),
        agent::CallResponse::Receipt { replayed: true, .. }
    ));
    ts::committed(&send(5, json!({"kind":"turn_ended","turn":1})));
    let container = ContainerRef::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    let head = fx.ctx.store.head(&id).unwrap();
    let turn = head.latest_agent_turn(&container, 1).unwrap().unwrap();
    let evidence = turn.provider_failure.as_ref().unwrap();
    assert_eq!(serde_json::to_value(evidence).unwrap(), failure);
    let stalled = stall(1, Some(&turn)).unwrap();
    assert_eq!(
        stalled.class.reset_at(stalled.at),
        Some("2027-01-15T08:10:00Z".parse().unwrap())
    );
    // Confirmed quota is immediate owner attention, not a same-candidate retry or paid launch.
    let change = advance_agent(
        &fx.ctx,
        &head,
        head.current_run.as_deref().unwrap(),
        stalled.at,
    )
    .unwrap()
    .unwrap();
    assert_eq!(change.to, Status::NeedsMe);
    assert!(fx.ctx.store.get(&id).unwrap().runs.len() == 1);
}

#[test]
fn reviewer_auth_failure_is_selected_from_real_current_generation_with_constant_work() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = f::fixture(false, 5);
        let target = f::reviewing(&fx);
        let container = target.container();
        let mut host = AgentRunState::new(AgentBinding {
            role: AgentRole::Reviewer,
            store: "fixture".into(),
            project: "corral".into(),
            generation: 2,
            health: HostHealth::Healthy,
            callback: None,
            closing: false,
        });
        let at: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        for n in 0..2 * scale {
            host.turns.push(serde_json::from_value(json!({"generation":1,"seq":n,"origin":{"kind":"unknown"},"started":at,"ended":null,"class":null,"provider_error":null})).unwrap());
            host.turns.push(serde_json::from_value(json!({"generation":2,"seq":n,"origin":{"kind":"unknown"},"started":at,"ended":at,"class":"completed","provider_error":null})).unwrap());
        }
        host.turns.push(serde_json::from_value(json!({"generation":2,"seq":9999,"origin":{"kind":"unknown"},"started":at,"ended":at,"class":"provider_failed","provider_error":"auth"})).unwrap());
        // A later append in an older generation cannot conceal current authentication failure.
        host.turns.push(serde_json::from_value(json!({"generation":1,"seq":99999,"origin":{"kind":"unknown"},"started":at,"ended":null,"class":null,"provider_error":null})).unwrap());
        fx.ctx
            .store
            .update(&fx.id, |task| {
                task.runs[0].review_runs[0].agent_host = Some(Box::new(host));
                Ok(())
            })
            .unwrap();
        let head = fx.ctx.store.head(&fx.id).unwrap();
        let hot = HotScope::enter();
        let before = work::snapshot();
        let Drive::Escalated(reason) = advance_reviewer(&fx.ctx, &head, &container, at).unwrap()
        else {
            panic!("authentication failure was hidden")
        };
        assert!(reason.contains("corral agent login") && reason.contains("no paid fallback"));
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        assert_eq!(fx.events("provider_escalated").len(), 1);
        assert!(fx.events("provider_retry").is_empty());
    }
    assert_eq!(measured[0], measured[1]);
}
