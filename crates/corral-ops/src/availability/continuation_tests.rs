//! Tiny native-state journeys; no provider process, pane inventory or network is involved.
use super::{checkpoint, continuation::*, policy};
use crate::agent::{BindTarget, CallResponse, test_support::*};
use corral_core::{
    AvailabilityFallback, CandidateCost, ExecutionCandidate, Profile, ProfileEligibility,
    ProfileMetadata, ProfileRole, SelectionGuidance, SpendingBound,
    agent::{AgentRole, AgentTurn, ContainerRef, HostHealth, TurnClass},
    availability::{FailureCategory, FailureScope, ProviderFailure},
    connection::{ConnectionBinding, ConnectionId},
    paid_episode::{PaidBinding, PaidPhase},
};
use serde_json::json;
use std::collections::BTreeSet;

const ROLES: [AgentRole; 6] = [
    AgentRole::Worker,
    AgentRole::Reviewer,
    AgentRole::Planner,
    AgentRole::PlanReviewer,
    AgentRole::Pm,
    AgentRole::Assistant,
];

pub(super) fn configure(
    fx: &mut Fx,
    paid_roles: BTreeSet<AgentRole>,
) -> (ConnectionBinding, ConnectionBinding) {
    let candidate = |id: &str, connection: &str, cost| ExecutionCandidate {
        id: id.into(),
        harness: "corral-pi".into(),
        connection: connection.into(),
        provider: "openai".into(),
        model: "same-model".into(),
        effort: None,
        args: vec![],
        evidence: "synthetic-owner-approval".into(),
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
        candidates: vec![sub.clone(), api.clone()],
        fallback: Some(AvailabilityFallback {
            automatic: true,
            paid_roles,
            approved_providers: BTreeSet::from(["openai".into()]),
            max_switches: 1,
            allow_unknown_subscription_cost: true,
            spending: Some(SpendingBound {
                currency: "USD".into(),
                maximum_micros: 1,
            }),
            approval: "synthetic-owner-approval".into(),
        }),
    };
    let profile: Profile = serde_json::from_value(json!({
        "harness":"corral-pi", "vendor":"openai", "model":"same-model", "metadata":metadata,
    }))
    .unwrap();
    fx.ctx.store.config.profiles.insert("pi".into(), profile);
    fx.ctx.store.save_config().unwrap();
    let resolve = |c: &ExecutionCandidate| {
        fx.ctx
            .store
            .config
            .resolve_candidate("pi", c)
            .unwrap()
            .connection_binding()
            .unwrap()
            .unwrap()
    };
    (resolve(&sub), resolve(&api))
}

struct Prepared {
    fx: Fx,
    task: String,
    source: Host,
    sub: ConnectionBinding,
    api: ConnectionBinding,
    subject: SubjectRevision,
    checkpoint: checkpoint::ContinuationCheckpoint,
    intent: ContinuationIntent,
}
fn prepared() -> Prepared {
    prepared_with_cap(None)
}
fn prepared_with_cap(cap: Option<u64>) -> Prepared {
    let mut fx = fx();
    let (sub, api) = configure(&mut fx, BTreeSet::from([AgentRole::Worker]));
    let task = worker_task(&fx, "p1");
    let container = ContainerRef::Worker {
        task: task.clone(),
        run: "R1".into(),
    };
    let source = bind(
        &fx,
        &BindTarget::Worker {
            task: task.clone(),
            run: "R1".into(),
        },
        "p1",
        "s1",
    );
    committed(&hello(&fx, &source));
    with_state(&fx.ctx, &container, |state| {
        state.generations[0].connection = Some(sub.clone());
        if let Some(cap) = cap {
            let mut config = fx.ctx.store.config.clone();
            config
                .agent
                .capabilities
                .set_settings(
                    AgentRole::Worker,
                    "budget",
                    std::collections::BTreeMap::from([("max_tokens".into(), json!(cap))]),
                )
                .map_err(anyhow::Error::msg)?;
            state.generations[0].capabilities =
                Some(config.agent.capabilities.resolve(AgentRole::Worker, None));
        }
        let now = corral_core::clock::now();
        let turn: AgentTurn = serde_json::from_value(json!({
            "generation":1, "seq":1, "origin":{"kind":"bridge"},
            "started":now, "ended":now, "class":"provider_failed",
        }))?;
        state.turns.push(AgentTurn {
            provider_failure: Some(ProviderFailure {
                version: 1,
                connection: ConnectionId::OpenaiSubscription,
                category: FailureCategory::SubscriptionQuota,
                scope: FailureScope::Account,
                retry_after_secs: None,
                reset_at: None,
                request_ref: None,
            }),
            ..turn
        });
        Ok(())
    })
    .unwrap();
    fx.ctx.store.update(&task, |task| {
        let now = corral_core::clock::now();
        let record = serde_json::from_value(json!({
            "artifact":"synthetic-checkpoint", "checkpoint":{
                "version":1, "revision":1, "captured":now, "progress":null,
                "completed_steps":[], "current_step":null, "input":{
                    "task":task.id, "run":"R1", "spec_revision":corral_core::store::spec_revision(task)?,
                    "expected_revision":0, "progress_revision":0, "head":"head", "base":"base", "tree":"tree",
                    "criterion_ids":[], "evidence_refs":[], "next_action":"continue", "decisions":[], "waits":[], "resources":[],
                }
            }
        }))?;
        task.runs[0].checkpoints.push(record);
        Ok(())
    }).unwrap();
    let subject = SubjectRevision {
        task: Some(task.clone()),
        spec_revision: None,
        target: Some("head:tree".into()),
    };
    let link = policy::ensure_root(&fx.ctx, &container, "pi", &subject).unwrap();
    assert_eq!(
        policy::ensure_root(&fx.ctx, &container, "pi", &subject).unwrap(),
        link
    );
    let state = state(&fx, &container);
    assert_eq!(state.turns[0].class, Some(TurnClass::ProviderFailed));
    let owner = PaidBinding {
        container: container.clone(),
        generation: 1,
        binding: sub.clone(),
    };
    let checkpoint = checkpoint::commit(
        &fx.ctx,
        &owner,
        &state,
        subject.clone(),
        b"committed worker handoff",
        checkpoint::receipt_refs(&state, 1).unwrap(),
    )
    .unwrap();
    let target = policy::target(&fx.ctx, "pi", AgentRole::Worker, &sub).unwrap();
    assert_eq!(target.binding, api);
    let intent = prepare(
        &fx.ctx,
        Transfer {
            source: container,
            successor: ContainerRef::Worker {
                task: task.clone(),
                run: "R2".into(),
            },
            target: api.clone(),
            context: ContextSource::Checkpoint {
                digest: checkpoint.digest().unwrap(),
            },
            subject: subject.clone(),
            reason: "not persisted: raw caller text".into(),
        },
    )
    .unwrap();
    Prepared {
        fx,
        task,
        source,
        sub,
        api,
        subject,
        checkpoint,
        intent,
    }
}
fn transfer(p: &Prepared, run: &str) -> Transfer {
    Transfer {
        source: p.intent.source.container.clone(),
        successor: ContainerRef::Worker {
            task: p.task.clone(),
            run: run.into(),
        },
        target: p.api.clone(),
        context: p.intent.context.clone(),
        subject: p.subject.clone(),
        reason: "subscription quota".into(),
    }
}
fn bind_successor(p: &Prepared) -> Host {
    p.fx.ctx
        .store
        .update(&p.task, |task| {
            task.runs.push(run("R2", "p2"));
            Ok(())
        })
        .unwrap();
    require_launch(&p.fx.ctx.store, &p.intent.successor.container, &p.api).unwrap();
    let host = bind(
        &p.fx,
        &BindTarget::Worker {
            task: p.task.clone(),
            run: "R2".into(),
        },
        "p2",
        "s2",
    );
    with_state(&p.fx.ctx, &p.intent.successor.container, |state| {
        state.generations[0].connection = Some(p.api.clone());
        Ok(())
    })
    .unwrap();
    host
}
fn paid_hello(p: &Prepared, host: &Host) -> CallResponse {
    hello_binding(&p.fx, host, &p.api)
}

pub(super) fn hello_binding(fx: &Fx, host: &Host, binding: &ConnectionBinding) -> CallResponse {
    let payload = json!({"binding_digest":binding.digest,
        "ctl_cap_sha256":corral_core::agent::paths::sha256_hex(host.ctl_cap.as_bytes()),
        "session_file":host.sessions.join("session.jsonl"), "pid":4242, "start_time":"123456", "pane":host.pane});
    send(
        fx,
        &envelope(
            host,
            "hello",
            &crate::agent::event_request_id(host.identity.generation, 0),
            payload,
            true,
        ),
        "hello",
    )
}

#[test]
fn failed_paid_start_pauses_once_and_revokes_launch_and_activation() {
    let p = prepared();
    bind_successor(&p);
    let successor = &p.intent.successor.container;
    assert!(failed_start(&p.fx.ctx, successor, 1).unwrap());
    let paused = intent_for(&p.fx.ctx.store, successor, 1).unwrap().unwrap();
    assert_eq!(paused.phase, IntentPhase::Paused);
    assert_eq!(paused.pause, Some(PauseReason::SuccessorAmbiguous));
    assert_eq!(state(&p.fx, successor).binding.health, HostHealth::Fenced);
    assert!(require_launch(&p.fx.ctx.store, successor, &p.api).is_err());
    assert!(activate(&p.fx.ctx, successor, 1).is_err());
    let before = task_bytes(&p.fx, &p.task);
    assert!(failed_start(&p.fx.ctx, successor, 1).unwrap());
    assert_eq!(task_bytes(&p.fx, &p.task), before);
}

#[test]
fn subscription_transfer_fences_replays_activates_and_rejects_ambiguous_successors() {
    let p = prepared();
    let before = task_bytes(&p.fx, &p.task);
    assert_eq!(prepare(&p.fx.ctx, transfer(&p, "R2")).unwrap(), p.intent);
    assert_eq!(task_bytes(&p.fx, &p.task), before);
    assert_eq!(
        state(&p.fx, &p.intent.source.container).binding.health,
        HostHealth::Fenced
    );
    assert!(
        state(&p.fx, &p.intent.source.container).generations[0]
            .fenced
            .is_some()
    );
    assert!(
        code(&send(
            &p.fx,
            &event_input(&p.source, 1, "turn_started", 2),
            "event"
        ))
        .is_some()
    );
    assert!(matches!(
        hello(&p.fx, &p.source),
        CallResponse::Receipt { replayed: true, .. }
    ));
    let canonical = p.fx.ctx.store.paid_episode(&p.intent.link.episode).unwrap();
    assert_eq!(canonical.switches, 1);
    let error = prepare(&p.fx.ctx, transfer(&p, "R3")).unwrap_err();
    assert!(error.to_string().contains("SuccessorAmbiguous"));
    assert_eq!(
        p.fx.ctx.store.paid_episode(&p.intent.link.episode).unwrap(),
        canonical
    );
    assert_eq!(
        intent_for(&p.fx.ctx.store, &p.intent.successor.container, 1).unwrap(),
        Some(p.intent.clone())
    );
    assert_eq!(task_bytes(&p.fx, &p.task), before);
    let wrong = ConnectionBinding::new(
        ConnectionId::OpenaiApi,
        Some("other".into()),
        "same-model".into(),
        None,
    )
    .unwrap();
    assert!(require_launch(&p.fx.ctx.store, &p.intent.successor.container, &wrong).is_err());
    require_preflight(&p.fx.ctx.store, AgentRole::Worker, &p.api).unwrap();
    assert!(require_preflight(&p.fx.ctx.store, AgentRole::Reviewer, &p.api).is_err());
    let host = bind_successor(&p);
    let hello = committed(&paid_hello(&p, &host));
    assert_eq!(hello.result["continuation"]["binding_digest"], p.api.digest);
    assert_eq!(
        hello.result["continuation"]["context"]["digest"],
        p.checkpoint.context.blob.hash().as_str()
    );
    let context = &hello.result["continuation"]["context"];
    let path = std::path::Path::new(context["path"].as_str().unwrap());
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(bytes.len(), p.checkpoint.context.bytes);
    assert_eq!(
        corral_core::blob_ref::BlobHash::of(&bytes),
        *p.checkpoint.context.blob.hash()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    assert_eq!(
        intent_for(&p.fx.ctx.store, &p.intent.successor.container, 1)
            .unwrap()
            .unwrap()
            .phase,
        IntentPhase::Active
    );
    assert!(!activate(&p.fx.ctx, &p.intent.successor.container, 1).unwrap());
    let active = p.fx.ctx.store.paid_episode(&p.intent.link.episode).unwrap();
    assert!(
        !close(
            &p.fx.ctx,
            &p.intent.source.container,
            "superseded source ended"
        )
        .unwrap()
    );
    assert_eq!(
        p.fx.ctx.store.paid_episode(&p.intent.link.episode).unwrap(),
        active
    );
    assert_eq!(
        state(&p.fx, &p.intent.successor.container).binding.health,
        HostHealth::Healthy
    );
    let input = envelope(
        &host,
        "event",
        "ev-1-1",
        json!({"kind":"budget", "action":"reserve",
        "request":"paid-request", "tokens":1, "usd_nanos":100, "priced":true,
        "provider":"openai", "model":"same-model"}),
        false,
    );
    assert_eq!(
        committed(&send(&p.fx, &input, "event")).result["admitted"],
        true
    );
    let episode = p.fx.ctx.store.paid_episode(&p.intent.link.episode).unwrap();
    assert_eq!(episode.ledger.totals().1, 100);
    assert_eq!(episode.switches, 1);
    assert_eq!(events(&p.fx, "continuation_prepared"), 1);
    assert_eq!(events(&p.fx, "continuation_switched"), 1);
    let before = task_bytes(&p.fx, &p.task);
    assert!(prepare(&p.fx.ctx, transfer(&p, "R3")).is_err());
    assert_eq!(
        p.fx.ctx.store.paid_episode(&p.intent.link.episode).unwrap(),
        episode
    );
    assert_eq!(task_bytes(&p.fx, &p.task), before);
    close(&p.fx.ctx, &p.intent.successor.container, "terminal result").unwrap();
    assert!(
        !path.exists(),
        "closed continuations must release raw context caches"
    );
    assert!(matches!(
        p.fx.ctx
            .store
            .paid_episode(&p.intent.link.episode)
            .unwrap()
            .phase,
        PaidPhase::Closed
    ));
    assert_eq!(events(&p.fx, "continuation_closed"), 1);
    assert_eq!(p.sub.connection, ConnectionId::OpenaiSubscription);
}

#[test]
fn all_container_kinds_refuse_unprepared_api_launches() {
    let fx = fx();
    let task = worker_task(&fx, "p1");
    let api = ConnectionBinding::new(
        ConnectionId::OpenaiApi,
        Some("api".into()),
        "same-model".into(),
        None,
    )
    .unwrap();
    let session = fx
        .ctx
        .store
        .create_assistant("pi", None, None, None)
        .unwrap();
    let containers = [
        ContainerRef::Worker {
            task: task.clone(),
            run: "R1".into(),
        },
        ContainerRef::Reviewer {
            task: task.clone(),
            run: "R1".into(),
            review: "child".into(),
        },
        ContainerRef::Planner {
            task: task.clone(),
            run: "D1".into(),
        },
        ContainerRef::PlanReviewer {
            task,
            plan: "P1".into(),
            review: "PR1".into(),
        },
        ContainerRef::Pm,
        ContainerRef::Assistant {
            session: session.id,
        },
    ];
    for container in containers {
        assert!(
            require_launch(&fx.ctx.store, &container, &api).is_err(),
            "{}",
            container.key()
        );
    }
    for role in ROLES {
        assert!(require_preflight(&fx.ctx.store, role, &api).is_err());
    }
}

#[test]
fn checkpoint_revalidates_subject_receipts_artifact_and_quiescence() {
    let p = prepared();
    let digest = p.checkpoint.digest().unwrap();
    let load = |subject: &SubjectRevision| {
        checkpoint::load(&p.fx.ctx.store, &digest, &p.intent.source, subject)
    };
    assert_eq!(load(&p.subject).unwrap().1, b"committed worker handoff");
    let mut changed = p.subject.clone();
    changed.target = Some("other:tree".into());
    assert!(load(&changed).is_err());
    let original = state(&p.fx, &p.intent.source.container);
    with_state(&p.fx.ctx, &p.intent.source.container, |s| {
        s.receipts[0].result = json!({"changed":true});
        Ok(())
    })
    .unwrap();
    assert!(load(&p.subject).is_err());
    with_state(&p.fx.ctx, &p.intent.source.container, |s| {
        s.receipts = original.receipts.clone();
        Ok(())
    })
    .unwrap();
    assert!(load(&p.subject).is_ok());
    let mut pending = original.clone();
    pending.turns[0].ended = None;
    assert!(
        checkpoint::commit(
            &p.fx.ctx,
            &p.intent.source,
            &pending,
            p.subject.clone(),
            b"ignored",
            vec![]
        )
        .is_err()
    );
    pending = original.clone();
    pending.receipts[0].state = corral_core::agent::ReceiptState::Uncertain;
    assert!(
        checkpoint::commit(
            &p.fx.ctx,
            &p.intent.source,
            &pending,
            p.subject.clone(),
            b"ignored",
            vec![]
        )
        .is_err()
    );
    pending = original.clone();
    pending.receipts[0]
        .post_pending
        .push("event:pending".into());
    assert!(
        checkpoint::commit(
            &p.fx.ctx,
            &p.intent.source,
            &pending,
            p.subject.clone(),
            b"ignored",
            vec![]
        )
        .is_err()
    );
    let path =
        p.fx.ctx
            .store
            .blob_store()
            .unwrap()
            .path_of(p.checkpoint.context.blob.hash());
    std::fs::remove_file(path).unwrap();
    assert!(load(&p.subject).is_err());
}

#[test]
fn default_empty_paid_roles_refuses_every_role_even_with_a_finite_approved_candidate() {
    let mut fx = fx();
    configure(&mut fx, BTreeSet::new());
    for role in ROLES {
        assert!(policy::approval(&fx.ctx, "pi", role).is_err());
    }
    let fallback = fx.ctx.store.config.profiles["pi"]
        .metadata
        .as_ref()
        .unwrap()
        .fallback
        .as_ref()
        .unwrap();
    assert!(
        serde_json::to_value(fallback)
            .unwrap()
            .get("paid_roles")
            .is_none()
    );
    let legacy: AvailabilityFallback = serde_json::from_value(json!({
        "automatic":true, "approved_providers":["openai"], "max_switches":1,
        "allow_unknown_subscription_cost":true, "spending":null, "approval":"synthetic",
    }))
    .unwrap();
    assert!(legacy.paid_roles.is_empty());
}

#[test]
fn activation_failure_preserves_committed_hello_but_pauses_without_a_spend_owner() {
    let p = prepared_with_cap(Some(1));
    let host = bind_successor(&p);
    with_state(&p.fx.ctx, &p.intent.successor.container, |state| {
        let mut config = p.fx.ctx.store.config.clone();
        config
            .agent
            .capabilities
            .set_settings(
                AgentRole::Worker,
                "budget",
                std::collections::BTreeMap::from([("max_tokens".into(), json!(2))]),
            )
            .map_err(anyhow::Error::msg)?;
        state.generations[0].capabilities =
            Some(config.agent.capabilities.resolve(AgentRole::Worker, None));
        Ok(())
    })
    .unwrap();
    committed(&paid_hello(&p, &host));
    let intent = intent_for(&p.fx.ctx.store, &p.intent.successor.container, 1)
        .unwrap()
        .unwrap();
    assert_eq!(intent.phase, IntentPhase::Paused);
    assert_eq!(
        state(&p.fx, &p.intent.successor.container).binding.health,
        HostHealth::Fenced
    );
    assert!(matches!(
        p.fx.ctx
            .store
            .paid_episode(&intent.link.episode)
            .unwrap()
            .phase,
        PaidPhase::Prepared { .. }
    ));
    assert_eq!(events(&p.fx, "continuation_switched"), 0);
    assert_eq!(events(&p.fx, "continuation_paused"), 1);
}

#[test]
fn revoking_prepared_successor_refuses_launch_activation_and_reservation() {
    let p = prepared();
    let host = bind_successor(&p);
    // Materialize the native bind/activation crash boundary: hello has made the host
    // healthy, but the independent financial activation has not happened.
    with_state(&p.fx.ctx, &p.intent.successor.container, |state| {
        let mut bound = state_for_fixture_bound(&p);
        bound.host_cap_sha256 = corral_core::agent::paths::sha256_hex(host.host_cap.as_bytes());
        bound.ctl_cap_sha256 = corral_core::agent::paths::sha256_hex(host.ctl_cap.as_bytes());
        bound.session_id = host.identity.session_id.clone();
        bound.session_file = host.sessions.join("session.jsonl");
        state.generations[0].bound = Some(bound);
        state.binding.health = HostHealth::Healthy;
        Ok(())
    })
    .unwrap();
    let before = task_bytes(&p.fx, &p.task);
    let error = with_state(&p.fx.ctx, &p.intent.source.container, |state| {
        close_in(
            &p.fx.ctx.store,
            &p.intent.source.container,
            state,
            "task-only closure",
        )
    })
    .unwrap_err();
    assert!(error.to_string().contains("close_task_in"));
    assert_eq!(task_bytes(&p.fx, &p.task), before);
    assert!(
        close(
            &p.fx.ctx,
            &p.intent.source.container,
            "old source terminalized"
        )
        .unwrap()
    );
    assert_eq!(
        state(&p.fx, &p.intent.successor.container).binding.health,
        HostHealth::Fenced
    );
    assert!(
        !close(
            &p.fx.ctx,
            &p.intent.successor.container,
            "successor revoked"
        )
        .unwrap()
    );
    assert_eq!(
        intent_for(&p.fx.ctx.store, &p.intent.successor.container, 1)
            .unwrap()
            .unwrap()
            .phase,
        IntentPhase::Closed
    );
    assert!(require_launch(&p.fx.ctx.store, &p.intent.successor.container, &p.api).is_err());
    assert!(activate(&p.fx.ctx, &p.intent.successor.container, 1).is_err());
    let input = envelope(
        &host,
        "event",
        "ev-1-1",
        json!({"kind":"budget", "action":"reserve",
        "request":"revoked", "tokens":1, "usd_nanos":1, "priced":true,
        "provider":"openai", "model":"same-model"}),
        false,
    );
    assert_eq!(code(&send(&p.fx, &input, "event")), Some("agent_fenced"));
    let episode = p.fx.ctx.store.paid_episode(&p.intent.link.episode).unwrap();
    assert!(matches!(episode.phase, PaidPhase::Prepared { .. }));
    assert!(episode.ledger.requests.is_empty());
}

fn state_for_fixture_bound(p: &Prepared) -> corral_core::agent::GenerationBound {
    state(&p.fx, &p.intent.source.container).generations[0]
        .bound
        .clone()
        .unwrap()
}

#[test]
fn closing_task_source_revokes_uncreated_prepared_or_paused_successor() {
    for paused in [false, true] {
        let p = prepared();
        if paused {
            pause(
                &p.fx.ctx,
                &p.intent.source.container,
                PauseReason::UnsupportedHost,
                "launch not opened",
            )
            .unwrap();
        }
        assert_eq!(p.fx.ctx.store.get(&p.task).unwrap().runs.len(), 1);
        assert!(
            close(
                &p.fx.ctx,
                &p.intent.source.container,
                "owner cancelled source"
            )
            .unwrap()
        );
        let intent = intent_for(&p.fx.ctx.store, &p.intent.successor.container, 1)
            .unwrap()
            .unwrap();
        assert_eq!(intent.phase, IntentPhase::Closed);
        assert!(require_launch(&p.fx.ctx.store, &intent.successor.container, &p.api).is_err());
        assert!(activate(&p.fx.ctx, &intent.successor.container, 1).is_err());
        assert_eq!(p.fx.ctx.store.get(&p.task).unwrap().runs.len(), 1);
        let episode = p.fx.ctx.store.paid_episode(&intent.link.episode).unwrap();
        assert!(episode.owner(&intent.link, &intent.successor).is_err());
        assert!(episode.ledger.requests.is_empty());
    }
}

#[test]
fn missing_continuation_index_never_authorizes_launch_or_another_preparation() {
    for source_index in [true, false] {
        let p = prepared();
        let (area, key) = if source_index {
            (
                "role-continuation-sources",
                corral_core::agent::paths::sha256_hex(
                    &serde_json::to_vec(&p.intent.source.container).unwrap(),
                ),
            )
        } else {
            (
                "role-continuation-tasks",
                corral_core::agent::paths::sha256_hex(
                    &serde_json::to_vec(&(&p.task, AgentRole::Worker)).unwrap(),
                ),
            )
        };
        p.fx.ctx.store.delete_record(area, &key).unwrap();
        let before = task_bytes(&p.fx, &p.task);
        let episode = p.fx.ctx.store.paid_episode(&p.intent.link.episode).unwrap();
        assert!(require_launch(&p.fx.ctx.store, &p.intent.successor.container, &p.api).is_err());
        assert!(prepare(&p.fx.ctx, transfer(&p, "R3")).is_err());
        assert_eq!(task_bytes(&p.fx, &p.task), before);
        assert_eq!(p.fx.ctx.store.paid_episode(&episode.id).unwrap(), episode);
    }
}

#[test]
fn quota_evidence_accepts_unclassified_roles_without_reusing_stale_quota() {
    let now = corral_core::clock::now();
    let binding = ConnectionBinding::new(
        ConnectionId::OpenaiSubscription,
        None,
        "same-model".into(),
        None,
    )
    .unwrap();
    let mut state: corral_core::agent::AgentRunState = serde_json::from_value(json!({
        "version":1,
        "binding":{"role":"assistant","store":"fixture","project":"corral","generation":2,"health":"healthy"},
        "generations":[{
            "n":2,"bind_nonce_sha256":"nonce","pane":"p1","created":now,"connection":binding,
            "bound":{"host_cap_sha256":"host","ctl_cap_sha256":"control","session_id":"session",
                "session_file":"session.jsonl","pid":1,"start_time":"start","request_id":"hello","at":now}
        }],
        "turns":[{
            "generation":2,"seq":1,"origin":{"kind":"bridge"},"started":now,"ended":now,
            "provider_failure":{"version":1,"connection":"openai-subscription",
                "category":"subscription_quota","scope":"account"}
        }]
    }))
    .unwrap();
    for role in ROLES {
        state.binding.role = role;
        assert_eq!(
            policy::quota_evidence(&state, None).is_some(),
            !matches!(role, AgentRole::Worker | AgentRole::Reviewer),
            "{role:?}"
        );
    }
    state.binding.role = AgentRole::Assistant;
    let mut latest = state.turns[0].clone();
    latest.seq = 2;
    latest.provider_failure.as_mut().unwrap().category = FailureCategory::Transient;
    state.turns.push(latest);
    assert!(policy::quota_evidence(&state, None).is_none());
    let failure = state.turns[1].provider_failure.as_mut().unwrap();
    failure.category = FailureCategory::SubscriptionQuota;
    failure.connection = ConnectionId::AnthropicSubscription;
    assert!(policy::quota_evidence(&state, None).is_none());
    state.turns.pop();
    state.turns[0].generation = 1;
    assert!(policy::quota_evidence(&state, None).is_none());
}
