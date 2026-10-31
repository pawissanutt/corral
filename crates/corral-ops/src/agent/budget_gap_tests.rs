use super::test_support::*;
use super::*;
use corral_core::agent::AgentRole;
use serde_json::json;

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
