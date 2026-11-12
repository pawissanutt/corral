//! Offline, real bridge transactions; no inference or live herdr.
use super::test_support::*;
use super::*;
use corral_core::agent::{AgentRole, ContainerRef, RequestReceipt};
use serde_json::json;

#[test]
fn budget_concurrent_admission_replay_fence_and_adaptive_resume() {
    let mut fx = fx();
    let task = worker_task(&fx, "p1");
    let target = BindTarget::Worker {
        task: task.clone(),
        run: "R1".into(),
    };
    let mut settings = std::collections::BTreeMap::new();
    settings.insert("max_tokens".into(), json!(100));
    settings.insert("max_usd".into(), json!(0.0001));
    fx.ctx
        .store
        .config
        .agent
        .capabilities
        .set_settings(AgentRole::Worker, "budget", settings.clone())
        .unwrap();
    let host = bind(&fx, &target, "p1", "s1");
    committed(&hello(&fx, &host));
    let snapshot = fx
        .ctx
        .store
        .config
        .agent
        .capabilities
        .resolve(AgentRole::Worker, None);
    assert!(snapshot.features["budget"].enabled);
    let save_snapshot = |snapshot| {
        fx.ctx
            .store
            .update(&task, |t| {
                t.runs[0]
                    .agent_host
                    .as_mut()
                    .unwrap()
                    .generations
                    .last_mut()
                    .unwrap()
                    .capabilities = Some(snapshot);
                Ok(())
            })
            .unwrap()
    };
    save_snapshot(snapshot.clone());
    let request = |host: &Host, seq, action: &str, key: &str, response: Option<&str>| {
        envelope(
            host,
            "event",
            &format!("ev-{}-{seq}", host.identity.generation),
            json!({"kind":"budget", "action":action,
        "request":key, "response":response, "tokens":25, "usd_nanos":25000, "priced":true}),
            false,
        )
    };
    // Concurrent exact transactions contend on one container, not four local balances.
    std::thread::scope(|scope| {
        let handles: Vec<_> = (1..=4)
            .map(|i| {
                let input = request(&host, i, "reserve", &format!("request-{i}"), None);
                let fx = &fx;
                scope
                    .spawn(move || committed(&send(fx, &input, "event")).result["admitted"].clone())
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap(), true);
        }
    });
    let denied = request(&host, 5, "reserve", "denied", None);
    assert_eq!(
        committed(&send(&fx, &denied, "event")).result["admitted"],
        false
    );
    assert!(matches!(
        send(&fx, &denied, "event"),
        CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(
        state(&fx, &target.container()).budget.totals(),
        (100, 100000)
    );
    fence_generation(&fx.ctx.store, &target.container(), 1, "budget test").unwrap();
    assert!(
        code(&send(
            &fx,
            &request(&host, 6, "reserve", "stale", None),
            "event"
        ))
        .is_some()
    );
    let resumed = bind(&fx, &target, "p1", "copy-of-s1");
    committed(&hello(&fx, &resumed));
    save_snapshot(snapshot);
    assert_eq!(
        committed(&send(
            &fx,
            &request(&resumed, 1, "reserve", "unchanged", None),
            "event"
        ))
        .result["admitted"],
        false
    );
    // Current configuration does not change the active generation. A new explicit generation
    // carries the newly resolved snapshot and retains the entire old container ledger.
    fence_generation(
        &fx.ctx.store,
        &target.container(),
        2,
        "raise configured cap",
    )
    .unwrap();
    settings.insert("max_tokens".into(), json!(125));
    settings.insert("max_usd".into(), json!(0.000125));
    let mut config = fx.ctx.store.config.agent.capabilities.clone();
    config
        .set_settings(AgentRole::Worker, "budget", settings)
        .unwrap();
    let higher = bind(&fx, &target, "p1", "rewound-copy");
    committed(&hello(&fx, &higher));
    save_snapshot(config.resolve(AgentRole::Worker, None));
    let reserve = request(&higher, 1, "reserve", "higher", None);
    assert_eq!(
        committed(&send(&fx, &reserve, "event")).result["admitted"],
        true
    );
    committed(&send(
        &fx,
        &request(
            &higher,
            2,
            "settle",
            "higher",
            Some("provider-model-response"),
        ),
        "event",
    ));
    let saved = state(&fx, &target.container());
    assert_eq!(saved.budget.totals(), (125, 125000));
    assert_eq!(
        saved
            .budget
            .requests
            .iter()
            .filter(|r| r.response.is_none())
            .count(),
        4
    );
    println!(
        "four concurrent admissions; replay deduped; stale host fenced; unchanged resume stopped; raised resume retains 4 uncertain reservations"
    );
}

#[test]
fn pm_budget_settles_guard_only_pm_and_owner_resume_reopens_admission() {
    fn event(fx: &Fx, host: &Host, seq: u64, payload: serde_json::Value) -> RequestReceipt {
        committed(&send(
            fx,
            &envelope(
                host,
                "event",
                &event_request_id(host.identity.generation, seq),
                payload,
                false,
            ),
            "event",
        ))
    }
    fn settle(fx: &Fx, host: &Host, seq: u64, key: &str) {
        let reserved = event(
            fx,
            host,
            seq,
            json!({
                "kind":"budget", "action":"reserve", "request":key,
                "tokens":300_001, "priced":true,
            }),
        );
        assert_eq!(reserved.result["admitted"], true);
        event(
            fx,
            host,
            seq + 1,
            json!({
                "kind":"budget", "action":"settle", "request":key, "response":key,
                "tokens":260_001, "provider":"fixture", "model":"fixture",
                "usage":{"input":150_000, "cache_read":0, "cache_write":110_000, "output":1},
            }),
        );
    }
    let fx = fx();
    let pm = bind(
        &fx,
        &BindTarget::Pm {
            project: String::new(),
        },
        "pm",
        "pm-session",
    );
    committed(&hello(&fx, &pm));
    settle(&fx, &pm, 1, "pm-first");
    let guard = || {
        fx.ctx
            .store
            .record::<serde_json::Value>("pm-usage-guard", "current")
            .unwrap()
            .unwrap()
    };
    let before = guard();
    assert_eq!(before["value"]["observations"].as_array().unwrap().len(), 1);
    assert_eq!(before["value"]["consecutive_misses"], 0);
    // Legacy hosts omit purpose: intake defaults to the conversation lineage.
    assert_eq!(
        before["value"]["observations"][0]["purpose"],
        "conversation"
    );
    let worker_task = worker_task(&fx, "p1");
    let worker = bind(
        &fx,
        &BindTarget::Worker {
            task: worker_task,
            run: "R1".into(),
        },
        "p1",
        "worker-session",
    );
    committed(&hello(&fx, &worker));
    settle(&fx, &worker, 1, "worker");
    let (_, target) = reviewer_task(&fx);
    let reviewer = bind(&fx, &target, "p2", "reviewer-session");
    committed(&hello(&fx, &reviewer));
    settle(&fx, &reviewer, 1, "reviewer");
    assert_eq!(guard(), before);
    settle(&fx, &pm, 3, "pm-second");
    assert_eq!(guard()["value"]["consecutive_misses"], 1);
    assert_eq!(events(&fx, "pm_cache_miss_guard"), 0);
    settle(&fx, &pm, 5, "pm-third");
    assert_eq!(events(&fx, "pm_cache_miss_guard"), 1);
    let paused = crate::pm_usage_guard::health(&fx.ctx).unwrap();
    assert_eq!(paused["status"]["state"], "paused");
    assert_eq!(paused["last_ratio"], 0.0);
    let denied = event(
        &fx,
        &pm,
        7,
        json!({
            "kind":"budget", "action":"reserve", "request":"denied", "tokens":1,
        }),
    );
    assert_eq!(denied.result["admitted"], false);
    assert_eq!(denied.result["reason"], "cache_guard_paused");
    assert_eq!(denied.result["exhausted"], false);
    assert_eq!(state(&fx, &ContainerRef::Pm).budget.requests.len(), 3);
    assert!(crate::pm_usage_guard::resume(&fx.ctx).unwrap());
    assert!(!crate::pm_usage_guard::resume(&fx.ctx).unwrap());
    let resumed = event(
        &fx,
        &pm,
        8,
        json!({
            "kind":"budget", "action":"reserve", "request":"resumed", "tokens":1, "priced":true,
        }),
    );
    assert_eq!(resumed.result["admitted"], true);
}
