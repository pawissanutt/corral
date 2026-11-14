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
