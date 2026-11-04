use super::test_support::*;
use super::*;
use corral_core::agent::AgentRole;
use serde_json::json;

#[test]
fn pm_resume_with_a_bound_legacy_generation_refuses_a_hard_budget_reservation() {
    let mut fx = fx();
    let target = BindTarget::Pm {
        project: "corral".into(),
    };
    let old = bind(&fx, &target, "pm-pane", "old-session");
    committed(&hello(&fx, &old));
    corral_core::agent::pm_state::update(&fx.ctx.store, |pm| {
        pm.as_mut()
            .unwrap()
            .state
            .generation_mut(1)
            .unwrap()
            .capabilities
            .as_mut()
            .unwrap()
            .catalog = 6;
        Ok(((), true))
    })
    .unwrap();
    fx.ctx
        .store
        .config
        .agent
        .capabilities
        .set_settings(
            AgentRole::Pm,
            "budget",
            [("max_tokens".into(), json!(100))].into(),
        )
        .unwrap();
    let current = bind(&fx, &target, "pm-pane", "current-session");
    committed(&hello(&fx, &current));
    let before = state(&fx, &target.container());
    assert!(before.generation(1).unwrap().bound.is_some());
    assert!(before.generation(1).unwrap().fenced.is_some());
    assert!(
        before
            .current_generation()
            .unwrap()
            .capabilities
            .as_ref()
            .unwrap()
            .catalog
            >= 7
    );
    assert_eq!(
        before
            .current_generation()
            .unwrap()
            .capabilities
            .as_ref()
            .unwrap()
            .features["budget"]
            .settings["max_tokens"],
        100
    );

    let input = envelope(
        &current,
        "event",
        "ev-2-1",
        json!({
            "kind":"budget", "action":"reserve", "request":"next",
            "tokens":10, "priced":true
        }),
        false,
    );
    let reply = committed(&send(&fx, &input, "event"));
    assert_eq!(reply.result["admitted"], false);
    assert_eq!(reply.result["legacy_usage_gap"], true);
    assert!(state(&fx, &target.container()).budget.requests.is_empty());
}

#[test]
fn historical_unpriced_or_legacy_usage_cannot_claim_a_usd_cap() {
    let fx = fx();
    let task = worker_task(&fx, "p1");
    let target = BindTarget::Worker {
        task: task.clone(),
        run: "R1".into(),
    };
    let host = bind(&fx, &target, "p1", "s1");
    committed(&hello(&fx, &host));
    let mut config = fx.ctx.store.config.agent.capabilities.clone();
    let snapshot = config.resolve(AgentRole::Worker, None);
    let save = |snapshot| {
        fx.ctx
            .store
            .update(&task, |t| {
                t.runs[0].agent_host.as_mut().unwrap().generations[0].capabilities = Some(snapshot);
                Ok(())
            })
            .unwrap()
    };
    let mut legacy_unlimited = snapshot.clone();
    legacy_unlimited.catalog = 6;
    save(legacy_unlimited);
    let input = |seq, key, priced| {
        envelope(
            &host,
            "event",
            &format!("ev-1-{seq}"),
            json!({"kind":"budget", "action":"reserve", "request":key,
            "tokens":10, "usd_nanos":0, "priced":priced}),
            false,
        )
    };
    assert_eq!(
        committed(&send(&fx, &input(1, "unpriced", false), "event")).result["admitted"],
        true
    );
    let reply = committed(&send(&fx, &input(2, "legacy-next", true), "event"));
    assert_eq!(reply.result["admitted"], true);
    assert_eq!(reply.result["exhausted"], false);
    assert_eq!(reply.result["legacy_usage_gap"], true);
    save(snapshot);
    config
        .set_settings(
            AgentRole::Worker,
            "budget",
            [("max_usd".into(), json!(1))].into(),
        )
        .unwrap();
    let capped = config.resolve(AgentRole::Worker, None);
    save(capped.clone());
    let reply = committed(&send(&fx, &input(3, "priced", true), "event"));
    assert_eq!(reply.result["admitted"], false);
    assert_eq!(reply.result["pricing_gaps"], 1);
    // A legacy snapshot has no historical reservation guarantee, even if its transcript is copied.
    let mut legacy = capped;
    legacy.catalog = 6;
    save(legacy);
    let reply = committed(&send(&fx, &input(4, "after-legacy", true), "event"));
    assert_eq!(reply.result["admitted"], false);
    assert_eq!(reply.result["legacy_usage_gap"], true);
    assert_eq!(state(&fx, &target.container()).budget.requests.len(), 2);
}
