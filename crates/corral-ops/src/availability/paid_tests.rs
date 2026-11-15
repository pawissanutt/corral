use crate::agent::{BindTarget, test_support::*};
use crate::availability::{
    checkpoint,
    continuation::*,
    continuation_tests::{configure, hello_binding},
    policy,
};
use corral_core::{
    AvailabilityFallback, CandidateCost, Config, ExecutionCandidate, ExecutionSnapshot, Profile,
    ProfileEligibility, ProfileMetadata, ProfileRole, SelectionGuidance,
};
use corral_core::{
    agent::{AgentRole, ContainerRef, HostHealth},
    connection::{ConnectionBinding, ConnectionId},
    paid_episode::{PaidBinding, PaidPhase},
};
use serde_json::json;
use std::collections::BTreeSet;

#[test]
fn same_model_candidates_match_the_frozen_connection_not_list_order() {
    let mut config = Config::default_for(std::path::Path::new("/offline-fixture"));
    let candidate = |id: &str, connection: &str, cost| ExecutionCandidate {
        id: id.into(),
        harness: "corral-pi".into(),
        connection: connection.into(),
        provider: "openai".into(),
        model: "same-model".into(),
        effort: None,
        args: vec![],
        evidence: "synthetic".into(),
        cost,
    };
    let sub = candidate("sub", "openai-subscription", CandidateCost::Subscription);
    let api = candidate(
        "api",
        "openai-api",
        CandidateCost::Metered {
            currency: "USD".into(),
            maximum_micros: 1,
        },
    );
    let metadata = ProfileMetadata {
        display_name: "fixture".into(),
        purpose: "fixture".into(),
        guidance: SelectionGuidance::default(),
        instructions: "fixture".into(),
        eligibility: ProfileEligibility {
            enabled: true,
            automatic: true,
            roles: BTreeSet::from([ProfileRole::Implementation]),
            projects: BTreeSet::new(),
        },
        revision: "fixture".into(),
        evidence: Some("synthetic".into()),
        candidates: vec![sub, api.clone()],
        fallback: Some(AvailabilityFallback {
            automatic: true,
            paid_roles: BTreeSet::new(),
            approved_providers: BTreeSet::from(["openai".into()]),
            max_switches: 1,
            allow_unknown_subscription_cost: true,
            spending: Some(corral_core::SpendingBound {
                currency: "USD".into(),
                maximum_micros: 1,
            }),
            approval: "synthetic".into(),
        }),
    };
    let profile: Profile = serde_json::from_value(serde_json::json!({"harness":"corral-pi", "vendor":"openai", "model":"same-model", "metadata":metadata})).unwrap();
    config.profiles.insert("fixture".into(), profile);
    let mut run = crate::agent::test_support::run("R1", "fixture");
    run.harness = Some("corral-pi".into());
    run.model = Some("same-model".into());
    run.execution = Some(ExecutionSnapshot::new(
        &config,
        config.resolve_candidate("fixture", &api).unwrap(),
    ));
    assert_eq!(super::super::matching(&metadata, &run).unwrap().id, "api");
    run.execution = None;
    assert!(super::super::matching(&metadata, &run).is_none());
}
fn profile_source(fx: &Fx) -> ConnectionBinding {
    fx.ctx
        .store
        .config
        .resolve_profile(
            &corral_core::ProfileChoice {
                profile: Some("pi".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap()
        .connection_binding()
        .unwrap()
        .unwrap()
}

struct PreparedAssistant {
    fx: Fx,
    container: ContainerRef,
    host: Host,
    subscription: ConnectionBinding,
    api: ConnectionBinding,
}

fn prepared_assistant() -> PreparedAssistant {
    let mut fx = fx();
    let (_, api) = configure(&mut fx, BTreeSet::from([AgentRole::Assistant]));
    let subscription = profile_source(&fx);
    assert!(subscription.candidate.is_none());
    let session = fx
        .ctx
        .store
        .create_assistant("pi", None, None, None)
        .unwrap();
    let target = BindTarget::Assistant {
        session: session.id,
    };
    let container = target.container();
    let host = bind(&fx, &target, "owner-pane", "subscription-session");
    committed(&hello(&fx, &host));
    with_state(&fx.ctx, &container, |state| {
        state.generations[0].connection = Some(subscription.clone());
        let now = corral_core::clock::now();
        state.turns.push(serde_json::from_value(json!({
            "generation":1, "seq":1, "origin":{"kind":"bridge"}, "started":now, "ended":now,
            "class":"provider_failed", "provider_failure":{
                "version":1, "connection":"openai-subscription", "category":"subscription_quota", "scope":"account"
            }
        }))?);
        Ok(())
    }).unwrap();
    let chosen = policy::target(&fx.ctx, "pi", AgentRole::Assistant, &subscription).unwrap();
    assert_eq!(chosen.binding, api);
    let subject = SubjectRevision {
        task: None,
        spec_revision: None,
        target: None,
    };
    let root = policy::ensure_root(&fx.ctx, &container, "pi", &subject).unwrap();
    let source = PaidBinding {
        container: container.clone(),
        generation: 1,
        binding: subscription.clone(),
    };
    let snapshot = state(&fx, &container);
    let checkpoint = checkpoint::commit(
        &fx.ctx,
        &source,
        &snapshot,
        subject.clone(),
        b"public assistant context",
        checkpoint::receipt_refs(&snapshot, 1).unwrap(),
    )
    .unwrap();
    let intent = prepare(
        &fx.ctx,
        Transfer {
            source: container.clone(),
            successor: container.clone(),
            target: api.clone(),
            context: ContextSource::Checkpoint {
                digest: checkpoint.digest().unwrap(),
            },
            subject,
            reason: "subscription quota".into(),
        },
    )
    .unwrap();
    assert_eq!(intent.source.binding, subscription);
    assert_eq!(intent.source_link, root);
    assert_eq!(intent.successor.binding, api);
    PreparedAssistant {
        fx,
        container,
        host,
        subscription,
        api,
    }
}

#[test]
fn candidate_less_assistant_source_selects_prepares_launches_and_activates_without_rewriting() {
    let PreparedAssistant {
        fx,
        container,
        host,
        subscription,
        api,
    } = prepared_assistant();
    require_preflight(&fx.ctx.store, AgentRole::Assistant, &api).unwrap();
    require_launch(&fx.ctx.store, &container, &api).unwrap();
    let mut successor = host.clone();
    successor.identity.generation = 2;
    successor.identity.session_id = "api-session".into();
    successor.nonce = secret("api-bind", 2);
    successor.host_cap = secret("api-host", 2);
    successor.ctl_cap = secret("api-ctl", 2);
    // Materialize the retained launch boundary without invoking a provider or launcher.
    with_state(&fx.ctx, &container, |state| {
        let mut generation = state.generations[0].clone();
        generation.n = 2;
        generation.bound = None;
        generation.fenced = None;
        generation.event_seq = 0;
        generation.bind_nonce_sha256 =
            corral_core::agent::paths::sha256_hex(successor.nonce.as_bytes());
        generation.connection = Some(api.clone());
        state.generations.push(generation);
        state.binding.generation = 2;
        state.binding.health = HostHealth::Unbound;
        Ok(())
    })
    .unwrap();
    committed(&hello_binding(&fx, &successor, &api));
    let active = intent_for(&fx.ctx.store, &container, 2).unwrap().unwrap();
    assert_eq!(active.phase, IntentPhase::Active);
    assert_eq!(active.source.binding, subscription);
    let state = state(&fx, &container);
    assert_eq!(
        state.generations[0].connection.as_ref(),
        Some(&subscription)
    );
    assert_eq!(
        state.current_generation().unwrap().connection.as_ref(),
        Some(&api)
    );
    fx.ctx
        .store
        .paid_episode(&active.link.episode)
        .unwrap()
        .owner(&active.link, &active.successor)
        .unwrap();
    pause(
        &fx.ctx,
        &container,
        PauseReason::UnsupportedHost,
        "active host stopped",
    )
    .unwrap();
    assert!(!close(&fx.ctx, &container, "already financially closed").unwrap());
}

#[test]
fn candidate_less_source_refuses_missing_or_ambiguous_metadata_but_exact_sources_stay_exact() {
    let mut fx = fx();
    let (exact, api) = configure(&mut fx, BTreeSet::from([AgentRole::Assistant]));
    let source = profile_source(&fx);
    let missing = ConnectionBinding::new(
        ConnectionId::OpenaiSubscription,
        None,
        "other-model".into(),
        None,
    )
    .unwrap();
    let error = policy::target(&fx.ctx, "pi", AgentRole::Assistant, &missing)
        .err()
        .unwrap();
    assert!(error.to_string().starts_with("LegacyBinding"));
    let metadata = fx
        .ctx
        .store
        .config
        .profiles
        .get_mut("pi")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap();
    let mut duplicate = metadata.candidates[0].clone();
    duplicate.id = "other-subscription".into();
    metadata.candidates.insert(1, duplicate);
    fx.ctx.store.save_config().unwrap();
    let error = policy::target(&fx.ctx, "pi", AgentRole::Assistant, &source)
        .err()
        .unwrap();
    assert!(error.to_string().starts_with("LegacyBinding"));
    assert_eq!(
        policy::target(&fx.ctx, "pi", AgentRole::Assistant, &exact)
            .unwrap()
            .binding,
        api
    );
}

#[test]
fn closing_retained_container_revokes_unopened_prepared_or_paused_successor() {
    for paused in [false, true] {
        let PreparedAssistant {
            fx, container, api, ..
        } = prepared_assistant();
        if paused {
            pause(
                &fx.ctx,
                &container,
                PauseReason::UnsupportedHost,
                "launch failed before opening",
            )
            .unwrap();
        }
        assert_eq!(state(&fx, &container).generations.len(), 1);
        assert!(close(&fx.ctx, &container, "session ended before successor opened").unwrap());
        let intent = intent_for(&fx.ctx.store, &container, 2).unwrap().unwrap();
        assert_eq!(intent.phase, IntentPhase::Closed);
        assert!(require_launch(&fx.ctx.store, &container, &api).is_err());
        assert!(activate(&fx.ctx, &container, 2).is_err());
        let snapshot = state(&fx, &container);
        assert_eq!(snapshot.generations.len(), 1);
        assert_eq!(snapshot.binding.generation, 1);
        assert_eq!(snapshot.binding.health, HostHealth::Fenced);
        let episode = fx.ctx.store.paid_episode(&intent.link.episode).unwrap();
        assert!(matches!(episode.phase, PaidPhase::Prepared { .. }));
        assert!(episode.owner(&intent.link, &intent.successor).is_err());
        assert!(episode.ledger.requests.is_empty());
    }
}
