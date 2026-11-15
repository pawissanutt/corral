use super::*;
use crate::agent::BindTarget;
use crate::agent::bridge::fixture::{self as f, Fx, HostProc, SESSION};
use crate::agent::test_support as ts;
use corral_core::agent::paths;
use corral_core::connection::ConnectionBinding;
use corral_core::inbox::Recipient;
use serde_json::{Value, json};

fn advance_current(ctx: &Ctx, now: Timestamp) -> Result<bool> {
    let pm = pm_state::load_current(&ctx.store)?.context("fixture PM is missing")?;
    advance(ctx, &pm, now)
}

fn fixture() -> (Fx, ts::Host, HostProc, ConnectionBinding) {
    fixture_with_end(false)
}

fn fixture_with_end(cancelled: bool) -> (Fx, ts::Host, HostProc, ConnectionBinding) {
    let mut fx = crate::agent::hooks::pm::tests::fixture();
    let candidate = |id, connection, cost| {
        json!({
            "id":id, "harness":"corral-pi", "connection":connection, "provider":"openai",
            "model":"gpt-fixture", "args":[], "evidence":"synthetic-owner-approval", "cost":cost,
        })
    };
    let sub = candidate("sub", "openai-subscription", json!({"kind":"subscription"}));
    let api = candidate(
        "api",
        "openai-api",
        json!({"kind":"metered", "currency":"USD", "maximum_micros":1}),
    );
    fx.ctx.store.config.profiles.insert("pm-pi".into(), serde_json::from_value(json!({
        "harness":"corral-pi", "vendor":"openai", "model":"gpt-fixture", "connection":"openai-subscription", "metadata": {
            "display_name":"fixture", "purpose":"fixture", "guidance":{}, "instructions":"fixture",
            "eligibility":{"enabled":true,"automatic":true,"roles":["pm"],"projects":[]},
            "revision":"fixture", "evidence":"synthetic", "candidates":[sub,api],
            "fallback":{"automatic":true,"paid_roles":["pm"],"approved_providers":["openai"],
                "max_switches":1,"allow_unknown_subscription_cost":true,
                "spending":{"currency":"USD","maximum_micros":1},"approval":"synthetic-owner-approval"}
        }
    })).unwrap());
    fx.ctx.store.save_config().unwrap();
    // Profile changes invalidate the fixture's earlier operating-scope approval.
    let selection = corral_core::OperatingSelection {
        tasks: [fx.id.clone()].into(),
        ..Default::default()
    };
    let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
    fx.ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", fx.ctx.store.now())
        .unwrap();
    let config = &fx.ctx.store.config;
    let candidates = &config.profiles["pm-pi"]
        .metadata
        .as_ref()
        .unwrap()
        .candidates;
    let subscription = config
        .resolve_profile(
            &corral_core::ProfileChoice {
                profile: Some("pm-pi".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap()
        .connection_binding()
        .unwrap()
        .unwrap();
    assert!(subscription.candidate.is_none());
    let api = config
        .resolve_candidate("pm-pi", &candidates[1])
        .unwrap()
        .connection_binding()
        .unwrap()
        .unwrap();
    std::fs::write(config.agent.assets_dir.as_ref().unwrap().join("compat.json"),
        json!({"verdict":"GO","memory_projection":1,"connections":["openai-subscription","openai-api"]}).to_string()).unwrap();
    let host = HostProc::spawn(true);
    let bound = fx.bind(
        &BindTarget::Pm {
            project: String::new(),
        },
        "pm-pane",
        &host,
    );
    f::write_session(&bound, &f::session(&[], &[]));
    continuation::with_state(&fx.ctx, &ContainerRef::Pm, |state| {
        state.generations[0].connection = Some(subscription);
        let now = fx.ctx.store.now();
        state.receipts.push(ts::receipt(&bound, "owner-action", "reply", "committed", json!({"reply":"recorded"})));
        state.questions.push(serde_json::from_value(json!({
            "version":1,"key":"pm/Q1","revision":1,
            "asked":{"request_id":"question-request","payload_sha256":"fixture","session_id":SESSION,"generation":1,"at":now},
            "kind":"clarification","blocking":true,"text":"Which release?","state":"answered",
            "answer":{"by":"owner","text":"Release one","at":now,"message_id":"owner-answer-1"},
            "wake":{"route":"none","key":"fixture","state":"posted"}
        }))?);
        Ok(())
    }).unwrap();
    // Exercise the quota-only publication hook while the provider-failed turn is open.
    let failure: corral_core::availability::ProviderFailure = serde_json::from_value(json!({
        "version":1,"connection":"openai-subscription","category":"subscription_quota","scope":"account"
    })).unwrap();
    for (seq, payload) in [
        (
            1,
            json!({"kind":"turn_started","turn":1,"provider":"openai-codex","model":"gpt-fixture"}),
        ),
        (
            2,
            json!({"kind":"provider_error","turn":1,"class":failure.class(),"failure":failure}),
        ),
        (
            3,
            json!({"kind":"turn_ended","turn":1,"cancelled":cancelled}),
        ),
    ] {
        let request = crate::agent::event_request_id(1, seq);
        ts::committed(
            &crate::agent::call(
                &fx.ctx,
                "event",
                &ts::envelope(&bound, "event", &request, payload, false),
            )
            .unwrap(),
        );
    }
    assert!(SIGNAL.load(&fx.ctx.store, "latest").unwrap().unwrap().quota);
    (fx, bound, host, api)
}

#[test]
fn pm_continuation_retains_container_fences_generation_and_preserves_native_journals() {
    let (fx, _bound, mut host, api) = fixture();
    let before = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert!(advance_current(&fx.ctx, fx.ctx.store.now()).unwrap());
    let after = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert_eq!(after.pane, before.pane);
    assert_eq!(
        after.state.binding.generation,
        2,
        "{:?}",
        fx.events("continuation_paused")
    );
    assert!(after.state.generation(1).unwrap().fenced.is_some());
    assert_eq!(
        after
            .state
            .generations
            .iter()
            .filter(|g| g.fenced.is_none())
            .map(|g| g.n)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(
        after.state.generation(2).unwrap().connection.as_ref(),
        Some(&api)
    );
    assert_eq!(
        serde_json::to_vec(&after.state.receipts).unwrap(),
        serde_json::to_vec(&before.state.receipts).unwrap()
    );
    assert_eq!(
        serde_json::to_vec(&after.state.questions).unwrap(),
        serde_json::to_vec(&before.state.questions).unwrap()
    );
    let intent = continuation::intent_for(&fx.ctx.store, &ContainerRef::Pm, 2)
        .unwrap()
        .unwrap();
    assert_eq!(intent.phase, IntentPhase::Prepared);
    assert_eq!(intent.source.container, ContainerRef::Pm);
    assert_eq!(intent.successor.container, ContainerRef::Pm);
    let ContextSource::Checkpoint { digest } = &intent.context else {
        panic!("missing native checkpoint");
    };
    let (manifest, bytes) =
        checkpoint::load(&fx.ctx.store, digest, &intent.source, &subject()).unwrap();
    assert_eq!(
        manifest.receipts,
        checkpoint::receipt_refs(&before.state, 1).unwrap()
    );
    let context: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(context["version"], 1);
    let dir = crate::agent::launch::generation_dir(&fx.ctx.store, &Recipient::Pm, 2).unwrap();
    let bootstrap: Value = serde_json::from_slice(
        &std::fs::read(dir.join(crate::agent::launch::BOOTSTRAP_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(bootstrap["fresh_session"], true);
    assert_eq!(bootstrap["binding"]["digest"], api.digest);
    assert_eq!(fx.calls("pane run").len(), 1);
    assert!(host.reaped());
    // A failed-to-bind Prepared successor is fenced and revoked, never launched twice.
    assert!(require_recoverable(&fx.ctx, &after.state).is_err());
    assert_eq!(
        continuation::intent_for(&fx.ctx.store, &ContainerRef::Pm, 2)
            .unwrap()
            .unwrap()
            .phase,
        IntentPhase::Paused
    );
    assert_eq!(fx.calls("pane run").len(), 1);
}

#[test]
fn pm_continuation_queued_input_pauses_without_changing_generation_or_journals() {
    let (fx, bound, mut host, _) = fixture();
    let path = f::session_file(&bound);
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(&format!("{}\n", json!({"type":"custom","customType":"corral_queue","data":{
        "version":1,"session_id":SESSION,"bind":"fixture-bind","generation":1,"item":"q1",
        "event":"queued","origin":"pane","mode":"follow_up","text_sha256":"fixture","text_bytes":1
    }})));
    std::fs::write(path, text).unwrap();
    let before = pm_state::load(&fx.ctx.store).unwrap();
    assert!(!advance_current(&fx.ctx, fx.ctx.store.now()).unwrap());
    assert_eq!(pm_state::load(&fx.ctx.store).unwrap(), before);
    assert_eq!(fx.events("continuation_paused").len(), 1);
    assert!(!advance_current(&fx.ctx, fx.ctx.store.now()).unwrap());
    assert_eq!(fx.events("continuation_paused").len(), 1);
    assert!(fx.calls("pane run").is_empty());
    assert!(!host.reaped());
}

#[test]
fn pm_continuation_concurrent_advance_skips_the_recovery_in_progress() {
    let (fx, _bound, mut host, _) = fixture();
    let before = pm_state::load(&fx.ctx.store).unwrap();
    let winner = PmRecoveryLease::acquire(&fx.ctx).unwrap();
    let error = PmRecoveryLease::acquire(&fx.ctx).err().unwrap();
    assert!(error.downcast_ref::<PmRecoveryInProgress>().is_some());
    assert!(!advance_current(&fx.ctx, fx.ctx.store.now()).unwrap());
    assert_eq!(pm_state::load(&fx.ctx.store).unwrap(), before);
    assert!(fx.calls("pane run").is_empty());
    assert!(!host.reaped());
    drop(winner);
    assert!(advance_current(&fx.ctx, fx.ctx.store.now()).unwrap());
    assert_eq!(
        pm_state::load(&fx.ctx.store)
            .unwrap()
            .unwrap()
            .state
            .binding
            .generation,
        2,
        "{:?}",
        fx.events("continuation_paused")
    );
    assert!(host.reaped());
    assert_eq!(
        paths::store_id(&fx.ctx.store),
        pm_state::load(&fx.ctx.store)
            .unwrap()
            .unwrap()
            .state
            .binding
            .store
    );
}

#[test]
fn pm_continuation_removed_permission_still_pauses_an_existing_paid_root() {
    let (mut fx, _bound, mut host, _) = fixture();
    policy::ensure_root(&fx.ctx, &ContainerRef::Pm, "pm-pi", &subject()).unwrap();
    fx.ctx
        .store
        .config
        .profiles
        .get_mut("pm-pi")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap()
        .fallback
        .as_mut()
        .unwrap()
        .paid_roles
        .clear();
    fx.ctx.store.save_config().unwrap();
    let before = pm_state::load(&fx.ctx.store).unwrap();
    assert!(!advance_current(&fx.ctx, fx.ctx.store.now()).unwrap());
    assert_eq!(pm_state::load(&fx.ctx.store).unwrap(), before);
    let paused = fx.events("continuation_paused");
    assert_eq!(paused.len(), 1);
    assert_eq!(paused[0].detail["pause"], "policy_not_approved");
    assert!(fx.calls("pane run").is_empty());
    assert!(!host.reaped());
}

#[test]
fn pm_continuation_rechecks_sticky_quota_before_attaching_a_paid_root() {
    let (fx, _bound, mut host, _) = fixture();
    let before = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    continuation::with_state(&fx.ctx, &ContainerRef::Pm, |state| {
        state.turns.push(serde_json::from_value(json!({
            "generation":1,"seq":2,"origin":{"kind":"bridge"},"started":fx.ctx.store.now()
        }))?);
        // Ordinary events cannot clear the previous quota signal.
        record_signal(&fx.ctx, state)
    })
    .unwrap();
    record_signal(&fx.ctx, &before.state).unwrap();
    assert!(!advance_current(&fx.ctx, fx.ctx.store.now()).unwrap());
    assert!(
        pm_state::load_current(&fx.ctx.store)
            .unwrap()
            .unwrap()
            .state
            .paid_episode
            .is_none()
    );
    // The supervisor consumed the stale signal; idle passes never acquire recovery.
    let _policy = fx.ctx.store.try_lock("pm-policy").unwrap().unwrap();
    assert!(!advance_current(&fx.ctx, fx.ctx.store.now()).unwrap());
    assert!(fx.calls("pane run").is_empty());
    assert!(!host.reaped());
}

#[test]
fn pm_continuation_cancelled_quota_turn_drops_sticky_signal_without_spending() {
    let (fx, _bound, mut host, _) = fixture_with_end(true);
    let before = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert_eq!(
        before.state.turns[0].class,
        Some(corral_core::agent::TurnClass::Cancelled)
    );
    assert!(before.state.turns[0].provider_failure.is_some());
    assert!(before.state.paid_episode.is_none());
    assert!(SIGNAL.load(&fx.ctx.store, "latest").unwrap().unwrap().quota);

    assert!(!advance_current(&fx.ctx, fx.ctx.store.now()).unwrap());
    assert_eq!(pm_state::load(&fx.ctx.store).unwrap(), Some(before));
    assert!(!SIGNAL.load(&fx.ctx.store, "latest").unwrap().unwrap().quota);
    assert!(fx.events("continuation_paused").is_empty());
    assert!(fx.calls("pane run").is_empty());
    assert!(!host.reaped());

    // A consumed signal must not acquire recovery on subsequent idle passes.
    let _policy = fx.ctx.store.try_lock("pm-policy").unwrap().unwrap();
    assert!(!advance_current(&fx.ctx, fx.ctx.store.now()).unwrap());
}

#[test]
fn pm_continuation_disabled_probe_adds_no_native_or_record_work() {
    let fx = ts::fx();
    let binding = serde_json::from_value(json!({
        "role":"pm","store":paths::store_id(&fx.ctx.store),"project":"",
        "generation":1,"health":"healthy"
    }))
    .unwrap();
    let current = AgentPm::new("unused".into(), AgentRunState::new(binding));
    let now = fx.ctx.store.now();
    let before = corral_core::work::snapshot();
    assert!(!advance(&fx.ctx, &current, now).unwrap());
    let delta = corral_core::work::snapshot().since(&before);
    for counter in corral_core::work::Counter::ALL {
        assert_eq!(delta.cells(counter).total, 0, "{counter:?}");
    }
}
