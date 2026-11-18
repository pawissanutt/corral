//! Synthetic supervisor journey: no host process, provider, herdr server or paid traffic.
use std::collections::BTreeSet;
use std::os::unix::process::ExitStatusExt;

use corral_core::agent::write_permissions as wp;
use corral_core::agent::{AgentRole, GenerationBound, HostHealth, RequestIdentity};
use corral_core::assistant::{NewOwnerInput, OwnerInputSource};
use corral_core::connection::ConnectionBinding;
use corral_core::{
    AvailabilityFallback, CandidateCost, ExecutionCandidate, ProfileEligibility, ProfileMetadata,
    ProfileRole, SelectionGuidance, SpendingBound,
};
use serde_json::{Value, json};

use crate::agent::bridge::fixture::{self as f, Fx};
use crate::agent::hooks::pm::{Overrides, tests::fixture};
use crate::agent::launch::{Bootstrap, Prepared};
use crate::agent::ops::assistant as ops;
use crate::agent::test_support as ts;
use crate::availability::{checkpoint, continuation as shared};

const PANE: &str = "w1:p9";

fn setup() -> (Fx, Prepared, ConnectionBinding) {
    let mut fx = fixture();
    std::fs::write(fx.root().join(PANE), "").unwrap();
    let assets = fx.root().join("assets");
    std::fs::create_dir_all(assets.join("src/roles/assistant")).unwrap();
    std::fs::write(assets.join("src/roles/assistant/index.ts"), "export {}").unwrap();
    std::fs::write(
        assets.join("compat.json"),
        json!({
            "version":1, "verdict":"GO", "bun":{"range":"1.4.2"},
            "memory_projection":1, "source_boundary":1,
            "connections":["openai-subscription","openai-api"],
        })
        .to_string(),
    )
    .unwrap();
    let candidate = |id: &str, connection: &str, cost| ExecutionCandidate {
        id: id.into(),
        harness: "corral-pi".into(),
        connection: connection.into(),
        provider: "openai".into(),
        model: "gpt-fixture".into(),
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
        instructions: "fixture".into(),
        guidance: SelectionGuidance::default(),
        revision: "fixture".into(),
        evidence: Some("synthetic".into()),
        eligibility: ProfileEligibility {
            enabled: true,
            automatic: true,
            roles: BTreeSet::from([ProfileRole::Implementation]),
            projects: BTreeSet::new(),
        },
        candidates: vec![sub.clone(), api.clone()],
        api_start: None,
        fallback: Some(AvailabilityFallback {
            automatic: true,
            paid_roles: BTreeSet::from([AgentRole::Assistant]),
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
    fx.ctx
        .store
        .config
        .profiles
        .get_mut("pm-pi")
        .unwrap()
        .metadata = Some(metadata);
    fx.ctx
        .store
        .config
        .profiles
        .get_mut("pm-pi")
        .unwrap()
        .connection = Some(corral_core::connection::ConnectionId::OpenaiSubscription);
    fx.ctx.store.save_config().unwrap();
    let api = fx
        .ctx
        .store
        .config
        .resolve_candidate("pm-pi", &api)
        .unwrap()
        .connection_binding()
        .unwrap()
        .unwrap();
    let ctx = f::ctx_at(fx.root());
    let (_, prepared, _) =
        ops::open(&ctx, ctx.store.root(), PANE, &Overrides::default(), false).unwrap();
    (fx, prepared, api)
}

fn bootstrap(prepared: &Prepared) -> Bootstrap {
    serde_json::from_slice(&std::fs::read(&prepared.bootstrap).unwrap()).unwrap()
}

fn journals(ctx: &crate::Ctx) -> Vec<u8> {
    let c = ctx.store.get_assistant_container("A-1").unwrap();
    serde_json::to_vec(
        &json!({ "owner_inputs": c.owner_inputs, "questions": c.state.questions,
            "receipts": c.state.receipts, "workflow": c.workflow, "todo": c.state.todo,
            "requests": c.state.todo_requests, "grants": c.write_grants,
            "grants_issued": c.write_grants_issued, "permission_audit": c.permission_audit,
            "session": ctx.store.get_assistant("A-1").unwrap(),
        }),
    )
    .unwrap()
}

fn seed(ctx: &crate::Ctx, bootstrap: &Bootstrap, quota: bool, active: bool) -> ts::Host {
    let now = corral_core::clock::now();
    let file = bootstrap.session_dir.join("source.jsonl");
    std::fs::create_dir_all(&bootstrap.session_dir).unwrap();
    std::fs::write(&file, "{\"type\":\"session\",\"id\":\"same\"}\n").unwrap();
    let host = ts::Host {
        identity: RequestIdentity {
            store: bootstrap.store.clone(),
            project: bootstrap.project.clone(),
            role: AgentRole::Assistant,
            task: None,
            run: None,
            review: None,
            callback: None,
            assistant: Some("A-1".into()),
            session_id: "same".into(),
            generation: 1,
            turn: None,
            parent: None,
        },
        host_cap: ts::secret("host", 1),
        ctl_cap: ts::secret("ctl", 1),
        nonce: bootstrap.bind_nonce.clone(),
        pane: PANE.into(),
        sessions: bootstrap.session_dir.clone(),
    };
    ctx.store.update_assistant_container("A-1", |c| {
        c.state.binding.health = HostHealth::Healthy;
        c.state.generations[0].bound = Some(GenerationBound {
            host_cap_sha256: corral_core::agent::paths::sha256_hex(host.host_cap.as_bytes()),
            ctl_cap_sha256: corral_core::agent::paths::sha256_hex(host.ctl_cap.as_bytes()),
            session_id: "same".into(), session_file: file.clone(), pid: u32::MAX,
            start_time: "gone".into(), request_id: "ev-1-0".into(), at: now, herdr_seq_epoch: None,
        });
        c.state.turns.push(serde_json::from_value(json!({"generation":1,"seq":1,
            "origin":{"kind":"bridge"},"started":now,"ended":now,
            "class":if quota {"provider_failed"} else {"completed"},
            "provider_failure":if quota { json!({"version":1,"connection":"openai-subscription",
                "category":"subscription_quota","scope":"account"}) } else { Value::Null },
        }))?);
        c.journal_owner_input("A-1", NewOwnerInput { generation: 1, turn: Some(1), source: OwnerInputSource::Pane,
            text: "Continue the agreed work".into(), sha256: corral_core::agent::paths::sha256_hex(b"Continue the agreed work"),
            truncated: false, command: None, answers: None, request: "ev-1-1".into(), binding: None });
        c.state.questions.push(serde_json::from_value(json!({"version":1,"key":"A-1/Q1","revision":1,
            "asked":{"request_id":"rq-question","payload_sha256":"digest","session_id":"same","generation":1,"at":now},
            "kind":"clarification","blocking":false,"text":"Which scope?","state":"answered",
            "answer":{"by":"owner","text":"Original scope","at":now},
            "wake":{"route":"none","key":"A-1/Q1","state":"posted"}}))?);
        c.state.receipts.push(serde_json::from_value(json!({"version":1,"request_id":"rq-completed",
            "op":"owner_action","payload_sha256":"digest","identity":host.identity,"state":"committed",
            "result":{"ok":true},"at":now,"committed_at":now}))?);
        let permission = wp::NewPermission {
            generation: 1, turn: Some(1), request: "rq-permission".into(),
            tool: wp::WriteTool::Write,
            operation: wp::WriteOperation { kind: wp::OperationKind::NewFile, replacements: None },
            target: wp::PermissionPath { shown: "/tmp/safe/a".into(), canonical: "/tmp/safe/a".into() },
            digest: "d".repeat(64),
            invocation: wp::Invocation { generation: 1, id: "1".repeat(32), tool_call: "write-once".into() },
            choice: wp::PermissionChoice::Hour,
            scope: Some(wp::PermissionPath { shown: "/tmp/safe".into(), canonical: "/tmp/safe".into() }),
        };
        c.record_permission("A-1", permission, now).map_err(|_| anyhow::anyhow!("grants full"))?;
        if active {
            super::workflow::boundary::apply(&ctx.store, "A-1", c, 1,
                serde_json::from_value(json!({"action":"start","id":"active-write","tool":"write","authority_epoch":0}))?)?;
        }
        super::workflow::prepare_entry(&ctx.store, "A-1", c, "A-1/g1/i1")?;
        c.workflow.as_mut().unwrap().current.phase = corral_core::assistant_workflow::WorkflowPhase::Planning;
        c.state.todo = Some(serde_json::from_value(json!({"revision":1,"spec_revision":"native",
            "plan":"P1","phases":[{"id":"read","label":"Read","steps":[
                {"id":"inspect","label":"Inspect","status":{"state":"pending"}}]}]}))?);
        c.state.todo_requests = Some(Default::default());
        Ok(())
    }).unwrap();
    host
}

#[test]
fn assistant_quota_exit_retains_journals_and_fences_only_the_generation() {
    for (quota, active) in [(true, false), (true, true), (false, false)] {
        let (fx, prepared, api) = setup();
        let ctx = f::ctx_at(fx.root());
        let mut calls = 0;
        let mut before = Vec::new();
        let mut source_host = None;
        let board_before = ctx.store.get(&fx.id).unwrap();
        crate::agent::pi::supervise_with(
            &ctx,
            PANE,
            ops::claim(&ctx.store, "A-1").unwrap(),
            prepared,
            &Overrides::default(),
            false,
            |prepared| {
                calls += 1;
                let b = bootstrap(prepared);
                assert_eq!(b.assistant.as_deref(), Some("A-1"));
                if calls == 1 {
                    source_host = Some(seed(&ctx, &b, quota, active));
                    before = journals(&ctx);
                    Ok(std::process::ExitStatus::from_raw(
                        crate::agent::pi::CONTINUE_ASSISTANT_EXIT << 8,
                    ))
                } else {
                    assert_eq!(calls, 2);
                    assert_eq!(b.generation, 2);
                    assert_eq!(journals(&ctx), before);
                    let container = ctx.store.get_assistant_container("A-1").unwrap();
                    assert!(container.state.generation(1).unwrap().fenced.is_some());
                    assert!(container.state.generation(2).unwrap().fenced.is_none());
                    let reference = corral_core::agent::ContainerRef::Assistant {
                        session: "A-1".into(),
                    };
                    let intent = shared::intent_for(&ctx.store, &reference, 2).unwrap();
                    if quota && !active {
                        assert!(b.fresh_session && b.resume_session.is_none());
                        assert_eq!(b.binding.as_ref(), Some(&api));
                        let intent = intent.unwrap();
                        assert_eq!(intent.phase, shared::IntentPhase::Prepared);
                        assert_eq!(intent.source.binding.candidate, None);
                        assert_eq!(intent.source.container, intent.successor.container);
                        let shared::ContextSource::Checkpoint { digest } = intent.context else {
                            panic!("missing checkpoint")
                        };
                        let (_, bytes) =
                            checkpoint::load(&ctx.store, &digest, &intent.source, &intent.subject)
                                .unwrap();
                        let context: Value = serde_json::from_slice(&bytes).unwrap();
                        assert_eq!(context["owner_inputs"][0]["id"], "A-1/g1/i1");
                        assert_eq!(context["questions"][0]["key"], "A-1/Q1");
                        // Mark the synthetic bind healthy so the supervisor doesn't classify an unbound start as a crash.
                        let file = b.session_dir.join("paid.jsonl");
                        std::fs::write(&file, "{\"type\":\"session\",\"id\":\"paid\"}\n").unwrap();
                        ctx.store
                            .update_assistant_container("A-1", |c| {
                                c.state.generations[1].bound = c.state.generations[0].bound.clone();
                                let bound = c.state.generations[1].bound.as_mut().unwrap();
                                bound.session_id = "paid".into();
                                bound.session_file = file.clone();
                                c.state.binding.health = HostHealth::Healthy;
                                Ok(())
                            })
                            .unwrap();
                        assert!(shared::activate(&ctx, &reference, 2).unwrap());
                    } else {
                        assert!(!b.fresh_session && b.resume_session.is_some());
                        assert!(!b.binding.as_ref().is_some_and(|b| b.connection.api_key()));
                        assert!(intent.is_none());
                        assert_eq!(
                            super::workflow::boundary::activities(&ctx.store, "A-1")
                                .unwrap()
                                .len(),
                            usize::from(active)
                        );
                    }
                    // A stale host cannot execute the completed owner action or mutate the successor.
                    let old = source_host.as_ref().unwrap();
                    let response = crate::agent::call(
                        &ctx,
                        "todo",
                        &ts::envelope(
                            old,
                            "todo",
                            &crate::agent::tool_request_id("same", "late"),
                            json!({"op":"init","phases":[]}),
                            false,
                        ),
                    )
                    .unwrap();
                    assert_eq!(
                        response.rejection().map(|r| r.code),
                        Some(crate::agent::RejectCode::AgentFenced)
                    );
                    assert_eq!(journals(&ctx), before);
                    Ok(std::process::ExitStatus::from_raw(0))
                }
            },
        )
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(ctx.store.list_assistants().unwrap().len(), 1);
        assert_eq!(ctx.store.get(&fx.id).unwrap(), board_before);
        assert_eq!(journals(&ctx), before);
        let paused = ctx.store.events_of(None, &["continuation_paused"]).unwrap();
        assert_eq!(paused.len(), usize::from(active));
        if active {
            assert_eq!(paused[0].detail["pause"], "uncertain_effect");
        }
        if !quota {
            assert!(
                ctx.store
                    .events_of(None, &["continuation_prepared"])
                    .unwrap()
                    .is_empty()
            );
        }
        if quota && !active {
            // An idle paid host crash regenerates inside the same allowance and transcript.
            let reference = corral_core::agent::ContainerRef::Assistant {
                session: "A-1".into(),
            };
            let now = corral_core::clock::now();
            ctx.store
                .update_assistant_container("A-1", |c| {
                    c.state
                        .turns
                        .push(serde_json::from_value(json!({"generation":2,"seq":1,
                    "origin":{"kind":"bridge"},"started":now,"ended":now,"class":"completed"}))?);
                    Ok(())
                })
                .unwrap();
            let (_, recovered) = ops::resume(&ctx, "A-1", PANE, &Overrides::default()).unwrap();
            let b = bootstrap(&recovered);
            assert_eq!(b.generation, 3);
            assert_eq!(b.binding.as_ref(), Some(&api));
            assert!(!b.fresh_session && b.resume_session.is_some());
            let intent = shared::intent_for(&ctx.store, &reference, 3)
                .unwrap()
                .unwrap();
            assert_eq!(intent.context, shared::ContextSource::ResumedSession);
            assert_eq!(
                ctx.store
                    .paid_episode(&intent.link.episode)
                    .unwrap()
                    .switches,
                1
            );
            assert_eq!(journals(&ctx), before);
            ctx.store
                .update_assistant_container("A-1", |c| {
                    c.state.generations[2].bound = c.state.generations[1].bound.clone();
                    c.state.binding.health = HostHealth::Healthy;
                    Ok(())
                })
                .unwrap();
            assert!(shared::activate(&ctx, &reference, 3).unwrap());
            ops::end(&ctx, "A-1").unwrap();
            assert_eq!(
                shared::intent_for(&ctx.store, &reference, 3)
                    .unwrap()
                    .unwrap()
                    .phase,
                shared::IntentPhase::Closed
            );
        }
    }
}
