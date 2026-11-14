//! Offline native receipt path across three tiny containers in one role lineage. No panes,
//! network, activation configuration, real credentials or provider traffic.
use super::{test_support::*, *};
use corral_core::{
    agent::{AgentRole, ContainerRef, paths},
    connection::{ConnectionBinding, ConnectionId},
    paid_episode::*,
};
use serde_json::json;

#[test]
fn paid_budget_native_lineage_preserves_sdk_estimates_and_crash_reservations() {
    let fx = fx();
    let task = worker_task(&fx, "p1");
    let target = |run: &str| BindTarget::Worker {
        task: task.clone(),
        run: run.into(),
    };
    let source = bind(&fx, &target("R1"), "p1", "s1");
    committed(&hello(&fx, &source));
    let subscription = ConnectionBinding::new(
        ConnectionId::OpenaiSubscription,
        Some("sub".into()),
        "same-model".into(),
        None,
    )
    .unwrap();
    fx.ctx
        .store
        .update(&task, |t| {
            t.runs[0].agent_host.as_mut().unwrap().generations[0].connection =
                Some(subscription.clone());
            Ok(())
        })
        .unwrap();
    let event = |host: &Host, seq, action: &str, request: &str, nanos, unbounded| {
        envelope(
            host,
            "event",
            &format!("ev-1-{seq}"),
            json!({"kind":"budget", "action":action, "request":request, "tokens":10, "usd_nanos":nanos,
            "priced":true, "unbounded":unbounded, "provider":"openai", "model":"same-model",
            "response": if action == "settle" { Some("canonical-response") } else { None }}),
            false,
        )
    };
    committed(&send(
        &fx,
        &event(&source, 1, "reserve", "sdk-uncertain", 999, true),
        "event",
    ));
    let api = ConnectionBinding::new(
        ConnectionId::OpenaiApi,
        Some("approved-api".into()),
        "same-model".into(),
        None,
    )
    .unwrap();
    let owner = |run: &str, binding: ConnectionBinding| PaidBinding {
        container: target(run).container(),
        generation: 1,
        binding,
    };
    let sub_owner = owner("R1", subscription);
    let link = fx
        .ctx
        .store
        .create_paid_episode(
            PaidRoot {
                store: paths::store_id(&fx.ctx.store),
                role: AgentRole::Worker,
                subject: task.clone(),
                attempt: "R1".into(),
            },
            PaidApproval {
                policy_digest: fx
                    .ctx
                    .store
                    .config_layers()
                    .unwrap()
                    .config()
                    .unwrap()
                    .profile_setup_revision()
                    .unwrap(),
                approval: "synthetic-owner-approval".into(),
                allowance_nanos: 100,
                candidates: vec![api.clone()],
            },
            sub_owner.clone(),
        )
        .unwrap();
    fx.ctx
        .store
        .update(&task, |t| {
            let mut episode = fx.ctx.store.paid_episode(&link.episode)?;
            episode.activate(&link, &sub_owner, t.runs[0].agent_host.as_mut().unwrap())?;
            fx.ctx.store.save_paid_episode(&episode)
        })
        .unwrap();
    let mut active_link = link;
    let mut previous = sub_owner;
    let mut current_host = source;
    for (index, name, pane) in [(1usize, "R2", "p2"), (2, "R3", "p3")] {
        let next = owner(name, api.clone());
        let mut prepared = None;
        fx.ctx
            .store
            .update(&task, |t| {
                let mut episode = fx.ctx.store.paid_episode(&active_link.episode)?;
                prepared = Some(episode.prepare_transfer(
                    &active_link,
                    &previous,
                    next.clone(),
                    t.runs[index - 1].agent_host.as_mut().unwrap(),
                )?);
                t.runs.push(run(name, pane));
                fx.ctx.store.save_paid_episode(&episode)
            })
            .unwrap();
        let next_link = prepared.unwrap();
        assert!(
            code(&send(
                &fx,
                &event(&current_host, 9, "settle", "paid-uncertain", 1, false),
                "event"
            ))
            .is_some()
        );
        let host = bind(&fx, &target(name), pane, name);
        committed(&hello(&fx, &host));
        fx.ctx
            .store
            .update(&task, |t| {
                let state = t.runs[index].agent_host.as_mut().unwrap();
                state.generations[0].connection = Some(api.clone());
                let mut episode = fx.ctx.store.paid_episode(&next_link.episode)?;
                episode.activate(&next_link, &next, state)?;
                fx.ctx.store.save_paid_episode(&episode)
            })
            .unwrap();
        let (request, nanos, expected) = if index == 1 {
            ("paid-uncertain", 60, true)
        } else {
            ("too-expensive", 50, false)
        };
        let input = event(&host, 1, "reserve", request, nanos, false);
        let answer = committed(&send(&fx, &input, "event"));
        assert_eq!(answer.result["admitted"], expected);
        assert_eq!(answer.result["subscription_estimate_nanos"], 999);
        assert_eq!(answer.result["paid"]["consumed_or_reserved_nanos"], 60);
        assert!(matches!(
            send(&fx, &input, "event"),
            CallResponse::Receipt { replayed: true, .. }
        ));
        active_link = next_link;
        previous = next;
        current_host = host;
    }
    assert_eq!(
        committed(&send(
            &fx,
            &event(&current_host, 2, "reserve", "known", 40, false),
            "event"
        ))
        .result["admitted"],
        true
    );
    committed(&send(
        &fx,
        &event(&current_host, 3, "settle", "known", 20, false),
        "event",
    ));
    let canonical = fx.ctx.store.paid_episode(&active_link.episode).unwrap();
    assert_eq!(canonical.ledger.totals(), (20, 80));
    assert_eq!(canonical.ledger.requests.len(), 2);
    assert_eq!(canonical.ledger.requests[0].response, None);
    assert_eq!(canonical.switches, 1);
    assert_eq!(
        fx.ctx
            .store
            .paid_episode_for_root(&canonical.root)
            .unwrap()
            .unwrap()
            .id,
        canonical.id
    );
    assert!(
        fx.ctx
            .store
            .create_paid_episode(
                canonical.root.clone(),
                canonical.approval.clone(),
                previous.clone()
            )
            .is_err()
    );
    let mut reset = canonical.clone();
    reset.approval.allowance_nanos = 200;
    assert!(
        fx.ctx
            .store
            .records_transaction(|| fx.ctx.store.save_paid_episode(&reset))
            .is_err()
    );
    assert_eq!(fx.ctx.store.record_rows(AREA).unwrap().len(), 1);
    assert_eq!(
        state(
            &fx,
            &ContainerRef::Worker {
                task,
                run: "R3".into()
            }
        )
        .budget
        .requests
        .len(),
        3
    );
}

#[test]
fn paid_replacement_preserves_independent_money_and_token_caps() {
    let mut fx = fx();
    let task = worker_task(&fx, "p1");
    let target = |run: &str| BindTarget::Worker {
        task: task.clone(),
        run: run.into(),
    };
    fx.ctx
        .store
        .config
        .agent
        .capabilities
        .set_settings(
            AgentRole::Worker,
            "budget",
            std::collections::BTreeMap::from([
                ("max_tokens".into(), json!(100)),
                ("max_usd".into(), json!(0.0001)),
            ]),
        )
        .unwrap();
    let snapshot = fx
        .ctx
        .store
        .config
        .agent
        .capabilities
        .resolve(AgentRole::Worker, None);
    let source = bind(&fx, &target("R1"), "p1", "s1");
    committed(&hello(&fx, &source));
    let api = ConnectionBinding::new(
        ConnectionId::OpenaiApi,
        Some("approved-api".into()),
        "same-model".into(),
        None,
    )
    .unwrap();
    let owner = |run: &str| PaidBinding {
        container: target(run).container(),
        generation: 1,
        binding: api.clone(),
    };
    let link = fx
        .ctx
        .store
        .create_paid_episode(
            PaidRoot {
                store: paths::store_id(&fx.ctx.store),
                role: AgentRole::Worker,
                subject: task.clone(),
                attempt: "R1".into(),
            },
            PaidApproval {
                policy_digest: fx
                    .ctx
                    .store
                    .config_layers()
                    .unwrap()
                    .config()
                    .unwrap()
                    .profile_setup_revision()
                    .unwrap(),
                approval: "synthetic-owner-approval".into(),
                allowance_nanos: 200_000,
                candidates: vec![api.clone()],
            },
            owner("R1"),
        )
        .unwrap();
    fx.ctx
        .store
        .update(&task, |t| {
            let state = t.runs[0].agent_host.as_mut().unwrap();
            state.generations[0].connection = Some(api.clone());
            state.generations[0].capabilities = Some(snapshot.clone());
            let mut episode = fx.ctx.store.paid_episode(&link.episode)?;
            episode.activate(&link, &owner("R1"), state)?;
            fx.ctx.store.save_paid_episode(&episode)
        })
        .unwrap();
    let event = |host: &Host, seq, action: &str, request: &str, tokens, nanos| {
        committed(&send(
            &fx,
            &envelope(
                host,
                "event",
                &format!("ev-1-{seq}"),
                json!({"kind":"budget", "action":action, "request":request,
                    "tokens":tokens, "usd_nanos":nanos, "priced":true,
                    "provider":"openai", "model":"same-model",
                    "response":if action == "settle" { Some("settled-response") } else { None }}),
                false,
            ),
            "event",
        ))
    };
    assert_eq!(
        event(&source, 1, "reserve", "known", 40, 40_000).result["admitted"],
        true
    );
    event(&source, 2, "settle", "known", 20, 20_000);
    assert_eq!(
        event(&source, 3, "reserve", "uncertain", 30, 30_000).result["admitted"],
        true
    );
    let before = state(&fx, &target("R1").container()).budget;
    assert_eq!(before.totals(), (50, 50_000));
    assert_eq!(
        before.requests[0].response.as_deref(),
        Some("settled-response")
    );
    assert_eq!(before.requests[1].response, None);
    let mut next_link = None;
    fx.ctx
        .store
        .update(&task, |t| {
            let mut episode = fx.ctx.store.paid_episode(&link.episode)?;
            next_link = Some(episode.prepare_transfer(
                &link,
                &owner("R1"),
                owner("R2"),
                t.runs[0].agent_host.as_mut().unwrap(),
            )?);
            t.runs.push(run("R2", "p2"));
            fx.ctx.store.save_paid_episode(&episode)
        })
        .unwrap();
    let next_link = next_link.unwrap();
    let replacement = bind(&fx, &target("R2"), "p2", "s2");
    committed(&hello(&fx, &replacement));
    fx.ctx
        .store
        .update(&task, |t| {
            let state = t.runs[1].agent_host.as_mut().unwrap();
            state.generations[0].connection = Some(api.clone());
            state.generations[0].capabilities = Some(snapshot.clone());
            let mut episode = fx.ctx.store.paid_episode(&next_link.episode)?;
            episode.activate(&next_link, &owner("R2"), state)?;
            fx.ctx.store.save_paid_episode(&episode)
        })
        .unwrap();
    // Exercise a real lifecycle head rewrite before the replacement's next admission.
    fx.ctx
        .store
        .update_head(&task, |head| {
            head.put_agent_record(
                &target("R2").container(),
                corral_core::agent::NativeRecord::Turn(serde_json::from_value(json!({
                    "generation":1, "seq":1, "origin":{"kind":"bridge"},
                    "started":"2026-11-01T00:00:00Z"
                }))?),
            )
            .map(drop)
        })
        .unwrap();
    assert_eq!(state(&fx, &target("R2").container()).budget, before);
    // Each cap independently rejects a request, despite ample paid allowance.
    for (seq, request, tokens, nanos) in [
        (1, "over-tokens", 51, 1),
        (2, "over-money", 1, 50_001),
    ] {
        let result = event(&replacement, seq, "reserve", request, tokens, nanos).result;
        assert_eq!(result["admitted"], false);
        assert_eq!(result["tokens"], 50);
        assert_eq!(result["usd_nanos"], 50_000);
        assert_eq!(result["limits"]["max_tokens"], 100);
        assert_eq!(result["limits"]["max_usd_nanos"], 100_000);
        assert_eq!(result["paid"]["allowance_nanos"], 200_000);
        assert_eq!(result["paid"]["consumed_or_reserved_nanos"], 50_000);
    }
    // Exactly 50 tokens / 50,000 nanos remain under the independent container caps.
    let exact = event(&replacement, 3, "reserve", "remaining", 50, 50_000).result;
    assert_eq!(exact["admitted"], true);
    assert_eq!(exact["tokens"], 100);
    assert_eq!(exact["usd_nanos"], 100_000);
    assert_eq!(exact["exhausted"], true);
    assert_eq!(exact["paid"]["consumed_or_reserved_nanos"], 100_000);
    assert_eq!(
        event(&replacement, 4, "reserve", "exhausted", 1, 1).result["admitted"],
        false
    );
    let canonical = fx.ctx.store.paid_episode(&next_link.episode).unwrap();
    assert_eq!(canonical.ledger.totals(), (100, 100_000));
    assert_eq!(
        canonical.approval.allowance_nanos - canonical.ledger.totals().1,
        100_000
    );
}
