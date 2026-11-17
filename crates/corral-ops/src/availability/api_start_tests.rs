//! API-first regression: isolated stores only, no credentials, providers or panes.
use super::{
    continuation::*,
    continuation_tests::{ROLES, configure, hello_binding},
};
use crate::agent::{BindTarget, test_support::*};
use corral_core::{
    ApiStartApproval, ProfileChoice, SpendingBound,
    agent::{AgentRole, BudgetRequest, ContainerRef, HostHealth},
    connection::{ConnectionBinding, ConnectionId},
    paid_episode::PaidPhase,
};
use std::collections::BTreeSet;

fn configured() -> (Fx, ConnectionBinding) {
    let mut fx = fx();
    configure(&mut fx, BTreeSet::from(ROLES));
    let profile = fx.ctx.store.config.profiles.get_mut("pi").unwrap();
    profile.connection = Some(ConnectionId::OpenaiApi);
    let metadata = profile.metadata.as_mut().unwrap();
    metadata.candidates.remove(0);
    metadata.fallback = None;
    metadata.api_start = Some(ApiStartApproval {
        roles: BTreeSet::from(ROLES),
        spending: SpendingBound {
            currency: "USD".into(),
            maximum_micros: 1,
        },
        approval: "explicit-api-start".into(),
    });
    fx.ctx.store.save_config().unwrap();
    let resolved = fx
        .ctx
        .store
        .config
        .resolve_profile(
            &ProfileChoice {
                profile: Some("pi".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
    let preferred = &fx.ctx.store.config.profiles["pi"]
        .metadata
        .as_ref()
        .unwrap()
        .candidates[0];
    let exact = fx
        .ctx
        .store
        .config
        .resolve_candidate("pi", preferred)
        .unwrap()
        .connection_binding()
        .unwrap()
        .unwrap();
    assert_eq!(resolved.connection_binding().unwrap(), Some(exact.clone()));
    (fx, exact)
}

#[test]
fn api_start_approval_is_role_and_selected_profile_scoped_not_fallback_or_selection() {
    let (mut fx, binding) = configured();
    // Identical runtime IDs in an unapproved profile cannot borrow another approval.
    let mut unapproved = fx.ctx.store.config.profiles["pi"].clone();
    unapproved.metadata.as_mut().unwrap().api_start = None;
    fx.ctx
        .store
        .config
        .profiles
        .insert("unapproved".into(), unapproved);
    fx.ctx.store.save_config().unwrap();
    for role in ROLES {
        require_preflight(&fx.ctx.store, role, &binding, "pi").unwrap();
        assert!(require_preflight(&fx.ctx.store, role, &binding, "unapproved").is_err());
    }
    assert!(
        fx.ctx
            .store
            .record_rows(corral_core::paid_episode::AREA)
            .unwrap()
            .is_empty()
    );
    let task = worker_task(&fx, "p1");
    let worker = ContainerRef::Worker {
        task: task.clone(),
        run: "R1".into(),
    };
    assert!(prepare_launch(&fx.ctx, &worker, &binding, Some("unapproved")).is_err());
    assert!(
        fx.ctx
            .store
            .record_rows(corral_core::paid_episode::AREA)
            .unwrap()
            .is_empty()
    );
    prepare_launch(&fx.ctx, &worker, &binding, Some("pi")).unwrap();
    fx.ctx
        .store
        .update(&task, |t| {
            t.runs.push(run("R2", "p2"));
            Ok(())
        })
        .unwrap();
    let retry = ContainerRef::Worker {
        task,
        run: "R2".into(),
    };
    assert!(prepare_launch(&fx.ctx, &retry, &binding, Some("pi")).is_err());
    assert_eq!(
        fx.ctx
            .store
            .record_rows(corral_core::paid_episode::AREA)
            .unwrap()
            .len(),
        1
    );
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
    metadata.api_start.as_mut().unwrap().roles = BTreeSet::from([AgentRole::Assistant]);
    fx.ctx.store.save_config().unwrap();
    assert!(require_preflight(&fx.ctx.store, AgentRole::Worker, &binding, "pi").is_err());
    require_preflight(&fx.ctx.store, AgentRole::Assistant, &binding, "pi").unwrap();
    assert!(super::policy::approval(&fx.ctx, "pi", AgentRole::Assistant).is_err());
    let config = &fx.ctx.store.config;
    let profile = &config.profiles["pi"];
    let original = profile.metadata.as_ref().unwrap();
    for maximum_micros in [0, 9_007_199_254_741, u64::MAX] {
        let mut invalid = original.clone();
        invalid.api_start.as_mut().unwrap().spending.maximum_micros = maximum_micros;
        assert!(invalid.validate(config, profile).is_err());
    }
    let mut invalid = original.clone();
    invalid.api_start.as_mut().unwrap().spending.currency = "EUR".into();
    assert!(invalid.validate(config, profile).is_err());
    let mut invalid = original.clone();
    invalid.api_start.as_mut().unwrap().approval = "  ".into();
    assert!(invalid.validate(config, profile).is_err());
}

#[test]
fn api_first_hello_activates_one_sticky_episode_and_resume_cannot_refresh_exhausted_allowance() {
    let (fx, binding) = configured();
    let session = fx
        .ctx
        .store
        .create_assistant("pi", None, None, None)
        .unwrap();
    let container = ContainerRef::Assistant {
        session: session.id.clone(),
    };
    prepare_launch(&fx.ctx, &container, &binding, Some("pi")).unwrap();
    let prepared = intent_for(&fx.ctx.store, &container, 1).unwrap().unwrap();
    assert_eq!(prepared.source, prepared.successor);
    assert_eq!(prepared.context, ContextSource::ApiStart);
    assert_eq!(
        notice(&fx.ctx.store, &container, 1).unwrap().unwrap()["context"]["kind"],
        "api-start"
    );
    prepare_launch(&fx.ctx, &container, &binding, Some("pi")).unwrap();
    assert_eq!(
        intent_for(&fx.ctx.store, &container, 1).unwrap().unwrap(),
        prepared
    );
    let target = BindTarget::Assistant {
        session: session.id.clone(),
    };
    let host = bind(&fx, &target, "p1", "session");
    with_state(&fx.ctx, &container, |state| {
        state.generations[0].connection = Some(binding.clone());
        Ok(())
    })
    .unwrap();
    committed(&hello_binding(&fx, &host, &binding));
    let active = intent_for(&fx.ctx.store, &container, 1).unwrap().unwrap();
    assert_eq!(active.phase, IntentPhase::Active);
    assert_eq!(
        state(&fx, &container).paid_episode,
        Some(prepared.link.clone())
    );
    let rows = crate::agent::app_runtime::sessions(&fx.ctx).unwrap();
    assert_eq!(
        rows.iter()
            .find(|r| r.id == session.id)
            .unwrap()
            .connection
            .as_deref(),
        Some("openai-api")
    );
    let mut episode = fx.ctx.store.paid_episode(&prepared.link.episode).unwrap();
    assert!(matches!(episode.phase, PaidPhase::Active { .. }));
    // An idle host (no quota predecessor, even no first turn) resumes the same ledger.
    with_state(&fx.ctx, &container, |state| {
        state.binding.health = HostHealth::Fenced;
        Ok(())
    })
    .unwrap();
    prepare_launch(&fx.ctx, &container, &binding, Some("pi")).unwrap();
    let resumed = intent_for(&fx.ctx.store, &container, 2).unwrap().unwrap();
    assert_eq!(resumed.link.episode, prepared.link.episode);
    assert_eq!(resumed.context, ContextSource::ResumedSession);
    let grant = crate::agent::open_generation_bound(
        &fx.ctx.store,
        &target,
        "p2",
        false,
        None,
        Some(&binding),
        false,
    )
    .unwrap();
    let mut host = host.clone();
    host.identity.generation = grant.generation;
    // Pi does not persist a transcript before the first turn. An empty start may
    // get a new Pi session ID, but it must keep the canonical paid episode.
    host.identity.session_id = "empty-recovery".into();
    host.nonce = grant.bind_nonce;
    host.host_cap = secret("resumed-host", grant.generation);
    host.ctl_cap = secret("resumed-ctl", grant.generation);
    host.pane = "p2".into();
    committed(&hello_binding(&fx, &host, &binding));
    episode = fx.ctx.store.paid_episode(&prepared.link.episode).unwrap();
    assert!(
        episode
            .reserve(
                &resumed.link,
                &resumed.successor,
                BudgetRequest {
                    connection: Some(binding.connection),
                    request: "exhaustion".into(),
                    generation: 2,
                    tokens: 1,
                    usd_nanos: 1000,
                    priced: true,
                    unbounded: false,
                    response: None,
                }
            )
            .unwrap()
    );
    fx.ctx.store.save_paid_episode(&episode).unwrap();
    with_state(&fx.ctx, &container, |state| {
        state.binding.health = HostHealth::Fenced;
        Ok(())
    })
    .unwrap();
    assert!(prepare_launch(&fx.ctx, &container, &binding, Some("pi")).is_err());
    let after = fx.ctx.store.paid_episode(&prepared.link.episode).unwrap();
    assert_eq!(after.approval.allowance_nanos, 1000);
    assert_eq!(after.ledger.totals().1, 1000);
    assert_eq!(
        fx.ctx
            .store
            .record_rows(corral_core::paid_episode::AREA)
            .unwrap()
            .len(),
        1
    );
    let subscription = ConnectionBinding::new(
        ConnectionId::OpenaiSubscription,
        None,
        "same-model".into(),
        None,
    )
    .unwrap();
    assert!(require_launch(&fx.ctx.store, &container, &subscription).is_err());
}
