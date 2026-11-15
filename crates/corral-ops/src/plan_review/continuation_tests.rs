//! Native launch/protocol simulation only: no provider traffic or real herdr.
use crate::agent::{BindTarget, test_support as ts};
use crate::availability::{
    checkpoint,
    continuation::{self, ContextSource, IntentPhase},
    policy,
};
use corral_core::agent::{AgentTurn, ContainerRef, paths};
use corral_core::{PlanRunState, Status};
use serde_json::json;

const PLAN: &str = "---\nhuman_review:\n  problem: Define the widget\n  changes: [Define contract]\n  retained: [Read-only design]\ngoal: Define the widget\nnon_goals: [No implementation]\ndecisions: [Use one widget]\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance:\n  - scenario: Inspect widget\n    expected: Defined\nverification: [cargo test]\nowner_decisions: []\n---\nDesign details.\n";

fn fixture() -> (ts::Fx, String, ts::Host, ContainerRef) {
    let mut fx = ts::fx();
    fx.ctx.resource_probe = crate::resources::FixedResourceProbe::arc();
    fx.ctx.store.config.agents.clear();
    fx.ctx.store.config.profiles.clear();
    fx.ctx.store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(json!({"path":"."})).unwrap(),
    );
    let candidate = |id: &str, connection: &str, cost| {
        json!({
            "id":id,"harness":"corral-pi","connection":connection,"provider":"openai",
            "model":"gpt-6.1-sol","evidence":"synthetic-owner-approval","cost":cost
        })
    };
    let profile = serde_json::from_value(json!({
        "harness":"corral-pi","vendor":"openai","model":"gpt-6.1-sol",
        "metadata": {
            "display_name":"fixture","purpose":"fixture","guidance":{},"instructions":"fixture",
            "eligibility":{"enabled":true,"automatic":true,"roles":["planning","plan_review"],"projects":[]},
            "revision":"fixture","evidence":"synthetic",
            "candidates":[candidate("sub","openai-subscription",json!({"kind":"subscription"})),
                candidate("api","openai-api",json!({"kind":"metered","currency":"USD","maximum_micros":1}))],
            "fallback":{"automatic":true,"paid_roles":["planner","plan_reviewer"],"approved_providers":["openai"],
                "max_switches":1,"allow_unknown_subscription_cost":true,"spending":{"currency":"USD","maximum_micros":1},"approval":"synthetic-owner-approval"}
        }
    })).unwrap();
    fx.ctx.store.config.profiles.insert("pi".into(), profile);
    fx.ctx.store.config.defaults.profile = Some("pi".into());
    fx.ctx.store.config.pm.agent = "pi".into();
    fx.ctx.store.config.review.reviewer = Some("pi".into());
    fx.ctx.store.config.dispatch.paused = false;
    fx.ctx.store.config.review.autopilot = Some(true);
    fx.ctx.store.config.limits.review = 1;
    fx.ctx.store.config.limits.count_reviews = false;
    let assets = fx._tmp.path().join("assets");
    for (name, content) in [
        ("package.json", "{\"name\":\"corral-pi\"}"),
        ("bun.lock", "{}"),
        (
            "compat.json",
            "{\"version\":1,\"verdict\":\"GO\",\"bun\":{\"range\":\"1.4.2\"},\"capabilities\":[\"role_enabled\"],\"connections\":[\"openai-subscription\",\"openai-api\"]}",
        ),
        ("fixture.json", "{\"fixture\":true}"),
        (crate::agent::launch::HOST_ENTRY, "// inert fixture host"),
        ("src/roles/planner/index.ts", "export default {};"),
        ("src/roles/plan-reviewer/index.ts", "export default {};"),
        ("src/delivery/index.ts", "export default {};"),
        ("src/auth/index.ts", "export default {};"),
    ] {
        let path = assets.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    fx.ctx.store.config.agent.assets_dir = Some(assets);
    let engine = fx._tmp.path().join("engine");
    std::fs::create_dir(&engine).unwrap();
    let bin = engine.join("herdr");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/plan_engine.sh"),
        &bin,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    fx.ctx.herdr = corral_herdr::Herdr::with_bin(bin, Some("continuation-fixture".into()));
    fx.ctx.store.save_config().unwrap();
    let id = fx
        .ctx
        .store
        .create("Design", Some("corral"), Status::Running, "Define widget")
        .unwrap()
        .id;
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [id.clone()].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", corral_core::clock::now())
        .unwrap();
    let socket = fx.ctx.herdr.socket_path().unwrap();
    let output = fx._tmp.path().join("draft-D1");
    std::fs::create_dir_all(&output).unwrap();
    let config = &fx.ctx.store.config;
    let resolved = config
        .resolve_candidate("pi", config.execution_candidate("pi", "sub").unwrap())
        .unwrap();
    fx.ctx
        .store
        .update(&id, |t| {
            t.profile = Some("pi".into());
            t.plan_runs.push(serde_json::from_value(json!({
            "id":"D1","profile":"pi","harness":"corral-pi","model":"gpt-6.1-sol","vendor":"openai",
            "execution":corral_core::ExecutionSnapshot::new(config, resolved.clone()),
            "spec_revision":crate::plan_spec_revision(t)?,"started":corral_core::clock::now(),
            "output_dir":output,"rounds":0,"state":"planning","delivery":"inbox",
            "herdr":{"workspace_id":"w1","pane_id":"w1:p1","socket":socket}
        }))?);
            Ok(())
        })
        .unwrap();
    let source = ContainerRef::Planner {
        task: id.clone(),
        run: "D1".into(),
    };
    let host = ts::bind(
        &fx,
        &BindTarget::Planner {
            task: id.clone(),
            run: "D1".into(),
        },
        "w1:p1",
        "planner-source",
    );
    ts::committed(&ts::hello(&fx, &host));
    (fx, id, host, source)
}

fn quota(fx: &ts::Fx, source: &ContainerRef) -> policy::QuotaEvidence {
    continuation::with_state(&fx.ctx, source, |state| {
        let config = &fx.ctx.store.config;
        state.generations[0].connection = config.resolve_candidate("pi", config.execution_candidate("pi", "sub")?)?.connection_binding()?;
        let now = corral_core::clock::now();
        let turn: AgentTurn = serde_json::from_value(json!({
            "generation":1,"seq":1,"origin":{"kind":"bridge"},"started":now,"ended":now,
            "provider_failure":{"version":1,"connection":"openai-subscription","category":"subscription_quota","scope":"account"}
        }))?;
        state.turns.push(turn);
        Ok(())
    }).unwrap();
    policy::quota_evidence(&ts::state(fx, source), None).unwrap()
}

fn host_from_bootstrap(fx: &ts::Fx, container: &ContainerRef) -> ts::Host {
    use crate::agent::launch::{BOOTSTRAP_FILE, Bootstrap, generation_dir};
    let recipient = match container {
        ContainerRef::Planner { task, run } => corral_core::inbox::Recipient::PlanRun {
            task: task.clone(),
            plan_run: run.clone(),
        },
        ContainerRef::PlanReviewer { task, plan, review } => {
            corral_core::inbox::Recipient::PlanReview {
                task: task.clone(),
                plan: plan.clone(),
                review: review.clone(),
            }
        }
        _ => panic!("planning container"),
    };
    let b: Bootstrap = serde_json::from_slice(
        &std::fs::read(
            generation_dir(&fx.ctx.store, &recipient, 1)
                .unwrap()
                .join(BOOTSTRAP_FILE),
        )
        .unwrap(),
    )
    .unwrap();
    let tag = format!(
        "successor-{}",
        &paths::sha256_hex(container.key().as_bytes())[..12]
    );
    ts::Host {
        identity:serde_json::from_value(json!({"store":b.store,"project":b.project,"role":b.role,"task":b.task,"run":b.run,"review":b.review,"callback":b.callback,"assistant":b.assistant,"session_id":tag,"generation":b.generation})).unwrap(),
        host_cap:ts::secret(&format!("{tag}-host"),1),ctl_cap:ts::secret(&format!("{tag}-ctl"),1),nonce:b.bind_nonce,pane:b.pane,sessions:b.session_dir
    }
}

#[test]
fn planner_continuation_checkpoints_committed_draft_or_starts_read_only() {
    for (committed_draft, recovery) in [
        (true, None),
        (false, None),
        (false, Some("resume")),
        (false, Some("refused")),
        (false, Some("owner-cancel")),
    ] {
        let (fx, id, old, source) = fixture();
        let request = crate::agent::tool_request_id(&old.identity.session_id, "draft");
        let write = ts::envelope(&old, "plan_write", &request, json!({"content":PLAN}), false);
        if committed_draft {
            ts::committed(&ts::send(&fx, &write, "plan_write"));
        }
        let evidence = quota(&fx, &source);
        let before = fx.ctx.store.get(&id).unwrap();
        if matches!(recovery, Some("refused" | "owner-cancel")) {
            let subject = continuation::SubjectRevision {
                task: Some(id.clone()),
                spec_revision: Some(before.plan_runs[0].spec_revision.clone()),
                target: None,
            };
            let state = ts::state(&fx, &source);
            let binding = state
                .current_generation()
                .unwrap()
                .connection
                .as_ref()
                .unwrap();
            let target = policy::target(
                &fx.ctx,
                "pi",
                corral_core::agent::AgentRole::Planner,
                binding,
            )
            .unwrap();
            policy::ensure_root(&fx.ctx, &source, "pi", &subject).unwrap();
            let successor = ContainerRef::Planner {
                task: id.clone(),
                run: "D2".into(),
            };
            continuation::prepare(
                &fx.ctx,
                continuation::Transfer {
                    source: source.clone(),
                    successor: successor.clone(),
                    target: target.binding,
                    context: ContextSource::FreshReadOnly {
                        reason: "no committed draft".into(),
                    },
                    subject,
                    reason: evidence.summary.clone(),
                },
            )
            .unwrap();
            let owner_cancel = recovery == Some("owner-cancel");
            if owner_cancel {
                crate::plan_cancel(&fx.ctx, &id).unwrap();
            } else {
                fx.ctx.store.pause_operating_policy(true).unwrap();
            }
            let before_dispatch = fx.ctx.store.get(&id).unwrap();
            let result = crate::plan_dispatch::dispatch(
                &fx.ctx,
                &fx.ctx.store.head(&id).unwrap(),
                &crate::DispatchOpts {
                    plan: true,
                    profile: Some("pi".into()),
                    ..Default::default()
                },
                &crate::dispatch::Admission::Automatic {
                    inputs: json!({"source":"availability"}),
                    at: corral_core::clock::now(),
                },
            );
            assert!(result.is_err());
            crate::plan_dispatch::continuation::paused(
                &fx.ctx,
                &source,
                result.map(|_| Vec::new()),
            )
            .unwrap();
            assert_eq!(fx.ctx.store.get(&id).unwrap(), before_dispatch);
            assert_eq!(before_dispatch.plan_runs.len(), 1);
            let intent = continuation::intent_for(&fx.ctx.store, &successor, 1)
                .unwrap()
                .unwrap();
            assert_eq!(
                intent.phase,
                if owner_cancel {
                    IntentPhase::Closed
                } else {
                    IntentPhase::Paused
                }
            );
            if !owner_cancel {
                assert_eq!(before_dispatch.status, before.status);
                assert_eq!(before_dispatch.plan_runs[0].state, PlanRunState::Planning);
            }
            continue;
        }
        crate::plan_dispatch::continuation::advance(
            &fx.ctx,
            &before,
            &before.plan_runs[0],
            &evidence,
            corral_core::clock::now(),
        )
        .unwrap();
        let after = fx.ctx.store.get(&id).unwrap();
        assert_eq!(
            after.plan_runs.len(),
            2,
            "{:?}",
            fx.ctx.store.events().unwrap().last()
        );
        assert_eq!(after.plan_runs[0].state, PlanRunState::Cancelled);
        assert!(after.plan_runs[0].ended.is_some());
        assert_ne!(after.plan_runs[0].output_dir, after.plan_runs[1].output_dir);
        let successor = ContainerRef::Planner {
            task: id.clone(),
            run: "D2".into(),
        };
        let intent = continuation::intent_for(&fx.ctx.store, &successor, 1)
            .unwrap()
            .unwrap();
        assert_eq!(intent.source.container, source);
        match intent.context {
            ContextSource::Checkpoint { digest } => {
                assert!(committed_draft);
                let (_, bytes) =
                    checkpoint::load(&fx.ctx.store, &digest, &intent.source, &intent.subject)
                        .unwrap();
                assert_eq!(bytes, PLAN.as_bytes());
            }
            ContextSource::FreshReadOnly { .. } => assert!(!committed_draft),
            other => panic!("unexpected {other:?}"),
        }
        let submit = ts::envelope(
            &old,
            "plan_submit",
            &crate::agent::tool_request_id(&old.identity.session_id, "late-submit"),
            json!({}),
            false,
        );
        assert!(ts::send(&fx, &submit, "plan_submit").rejection().is_some());
        if committed_draft {
            assert!(matches!(
                ts::send(&fx, &write, "plan_write"),
                crate::agent::CallResponse::Receipt { replayed: true, .. }
            ));
        }
        if recovery == Some("resume") {
            assert!(
                crate::plan_dispatch::resume_prepared(&fx.ctx, &fx.ctx.store.head(&id).unwrap())
                    .unwrap()
            );
            let paused = continuation::intent_for(&fx.ctx.store, &successor, 1)
                .unwrap()
                .unwrap();
            assert_eq!(paused.phase, IntentPhase::Paused);
            assert_eq!(
                paused.pause,
                Some(continuation::PauseReason::SuccessorAmbiguous)
            );
            assert_eq!(fx.ctx.store.get(&id).unwrap().plan_runs.len(), 2);
            assert!(
                continuation::require_launch(&fx.ctx.store, &successor, &paused.successor.binding)
                    .is_err()
            );
            continue;
        }
        let next = host_from_bootstrap(&fx, &successor);
        hello(&fx, &next, &successor);
        assert_eq!(
            continuation::intent_for(&fx.ctx.store, &successor, 1)
                .unwrap()
                .unwrap()
                .phase,
            IntentPhase::Active
        );
        if !committed_draft {
            crate::plan_cancel(&fx.ctx, &id).unwrap();
            assert_eq!(
                continuation::intent_for(&fx.ctx.store, &successor, 1)
                    .unwrap()
                    .unwrap()
                    .phase,
                IntentPhase::Closed
            );
            assert_eq!(
                fx.ctx.store.get(&id).unwrap().plan_runs[1].state,
                PlanRunState::Cancelled
            );
            continue;
        }
        let draft = ts::envelope(
            &next,
            "plan_write",
            &crate::agent::tool_request_id(&next.identity.session_id, "next-draft"),
            json!({"content":PLAN}),
            false,
        );
        ts::committed(&ts::send(&fx, &draft, "plan_write"));
        let submit = ts::envelope(
            &next,
            "plan_submit",
            &crate::agent::tool_request_id(&next.identity.session_id, "next-submit"),
            json!({}),
            false,
        );
        let receipt = ts::committed(&ts::send(&fx, &submit, "plan_submit"));
        assert_eq!(receipt.result["outcome"], "submitted");
        assert_eq!(
            continuation::intent_for(&fx.ctx.store, &successor, 1)
                .unwrap()
                .unwrap()
                .phase,
            IntentPhase::Closed
        );
    }
}

#[test]
fn plan_review_continuation_is_fresh_and_uncertain_verdict_pauses() {
    for uncertain in [false, true] {
        let (fx, id, _, _) = fixture();
        let path = fx._tmp.path().join("submitted.md");
        std::fs::write(&path, PLAN).unwrap();
        let plan = crate::plan_submit(&fx.ctx, &id, &path).unwrap();
        let digest =
            crate::plan::policy_digest(&fx.ctx.store.config, &fx.ctx.store.get(&id).unwrap())
                .unwrap();
        fx.ctx.store.update(&id, |t| {
            t.plans[0].reviews.push(serde_json::from_value(json!({"id":"PR1","reviewer":"pi","policy_digest":digest,"at":corral_core::clock::now(),"notes":"partial, non-authoritative","delivery":"inbox",
                "herdr":{"workspace_id":"w1","pane_id":"w1:p1","socket":fx.ctx.herdr.socket_path()?}}))?);
            t.plan_runs[0].state = PlanRunState::Reviewing;
            Ok(())
        }).unwrap();
        let source = ContainerRef::PlanReviewer {
            task: id.clone(),
            plan: plan.id.clone(),
            review: "PR1".into(),
        };
        let old = ts::bind(
            &fx,
            &BindTarget::PlanReviewer {
                task: id.clone(),
                plan: plan.id.clone(),
                review: "PR1".into(),
            },
            "w1:p1",
            "review-source",
        );
        ts::committed(&ts::hello(&fx, &old));
        let evidence = quota(&fx, &source);
        if uncertain {
            continuation::with_state(&fx.ctx,&source,|state| {
                state.receipts.push(serde_json::from_value(json!({"version":1,"request_id":"unknown-verdict","op":"plan_verdict","payload_sha256":"00","identity":old.identity,"state":"uncertain","result":{},"at":corral_core::clock::now()}))?);
                Ok(())
            }).unwrap();
        }
        let before = fx.ctx.store.get(&id).unwrap();
        let plan = &before.plans[0];
        super::continuation::advance(
            &fx.ctx,
            &before,
            plan,
            &plan.reviews[0],
            &evidence,
            corral_core::clock::now(),
        )
        .unwrap();
        let after = fx.ctx.store.get(&id).unwrap();
        if uncertain {
            assert_eq!(after, before);
            assert!(fx.ctx.store.events().unwrap().iter().any(
                |e| e.kind == "continuation_paused" && e.detail["pause"] == "uncertain_effect"
            ));
            continue;
        }
        assert_eq!(
            after.plans[0].reviews.len(),
            2,
            "{:?}",
            fx.ctx.store.events().unwrap().last()
        );
        assert!(after.plans[0].reviews[0].cancelled);
        assert!(after.plans[0].reviews[0].verdict.is_none());
        let next = &after.plans[0].reviews[1];
        assert_eq!(next.id, "PR2");
        assert!(next.verdict.is_none() && next.notes.is_none());
        let successor = ContainerRef::PlanReviewer {
            task: id.clone(),
            plan: plan.id.clone(),
            review: "PR2".into(),
        };
        let intent = continuation::intent_for(&fx.ctx.store, &successor, 1)
            .unwrap()
            .unwrap();
        assert_eq!(intent.source.container, source);
        assert!(matches!(
            intent.context,
            ContextSource::FreshReadOnly { .. }
        ));
        assert_eq!(
            after.plans[0].reviews[1]
                .agent_host
                .as_ref()
                .unwrap()
                .paid_episode
                .as_ref(),
            Some(&intent.link)
        );
        let next = host_from_bootstrap(&fx, &successor);
        hello(&fx, &next, &successor);
        let late = ts::envelope(
            &old,
            "plan_verdict",
            &crate::agent::tool_request_id(&old.identity.session_id, "late-verdict"),
            json!({"verdict":"approve","notes":"late"}),
            false,
        );
        assert!(ts::send(&fx, &late, "plan_verdict").rejection().is_some());
        let verdict = ts::envelope(
            &next,
            "plan_verdict",
            &crate::agent::tool_request_id(&next.identity.session_id, "new-verdict"),
            json!({"verdict":"changes","notes":"Fresh findings"}),
            false,
        );
        assert_eq!(
            ts::committed(&ts::send(&fx, &verdict, "plan_verdict")).result["outcome"],
            "recorded"
        );
        assert_eq!(
            continuation::intent_for(&fx.ctx.store, &successor, 1)
                .unwrap()
                .unwrap()
                .phase,
            IntentPhase::Closed
        );
    }
}

fn hello(fx: &ts::Fx, host: &ts::Host, container: &ContainerRef) {
    let state = ts::state(fx, container);
    let binding = state
        .current_generation()
        .unwrap()
        .connection
        .as_ref()
        .unwrap();
    let payload = json!({"binding_digest":binding.digest,
        "ctl_cap_sha256":paths::sha256_hex(host.ctl_cap.as_bytes()),
        "session_file":host.sessions.join("session.jsonl"),"pid":4242,"start_time":"123456","pane":host.pane});
    ts::committed(&ts::send(
        fx,
        &ts::envelope(host, "hello", "ev-1-0", payload, true),
        "hello",
    ));
}

#[test]
fn planning_sweep_reads_only_keyed_evidence_after_role_opt_in() {
    use crate::plan_dispatch::continuation::due;
    use corral_core::work::{Counter, HotScope, snapshot};

    let (mut fx, id, _, source) = fixture();
    let head = fx.ctx.store.head(&id).unwrap();
    {
        let _hot = HotScope::enter();
        let before = snapshot();
        assert!(policy::configured(
            &fx.ctx.store.config,
            corral_core::agent::AgentRole::Planner
        ));
        assert!(!due(&fx.ctx, &head).unwrap());
        assert_eq!(snapshot(), before);
    }
    quota(&fx, &source);
    let head = fx.ctx.store.head(&id).unwrap();
    {
        let _hot = HotScope::enter();
        let before = snapshot();
        assert!(due(&fx.ctx, &head).unwrap());
        let after = snapshot();
        assert_eq!(
            after.total(Counter::NestedRowDecodes) - before.total(Counter::NestedRowDecodes),
            2
        );
        assert_eq!(after.full_task_hydrations(), before.full_task_hydrations());
        assert_eq!(
            after.total(Counter::RecordDecodes),
            before.total(Counter::RecordDecodes)
        );
        assert_eq!(
            after.total(Counter::TaskHeadDecodes),
            before.total(Counter::TaskHeadDecodes)
        );
    }
    continuation::with_state(&fx.ctx, &source, |state| {
        state.turns[0].provider_failure.as_mut().unwrap().category =
            corral_core::FailureCategory::Transient;
        Ok(())
    })
    .unwrap();
    let head = fx.ctx.store.head(&id).unwrap();
    for paid in [true, false] {
        if !paid {
            fx.ctx
                .store
                .config
                .profiles
                .get_mut("pi")
                .unwrap()
                .metadata
                .as_mut()
                .unwrap()
                .fallback
                .as_mut()
                .unwrap()
                .paid_roles
                .clear();
        }
        let _hot = HotScope::enter();
        let before = snapshot();
        assert!(
            !(policy::configured(&fx.ctx.store.config, corral_core::agent::AgentRole::Planner)
                && due(&fx.ctx, &head).unwrap())
        );
        let after = snapshot();
        if paid {
            assert_eq!(
                after.total(Counter::NestedRowDecodes) - before.total(Counter::NestedRowDecodes),
                2
            );
            assert_eq!(after.full_task_hydrations(), before.full_task_hydrations());
            assert_eq!(
                after.total(Counter::RecordDecodes),
                before.total(Counter::RecordDecodes)
            );
            assert_eq!(
                after.total(Counter::TaskHeadDecodes),
                before.total(Counter::TaskHeadDecodes)
            );
        } else {
            assert_eq!(after, before);
        }
    }
}
#[path = "start_failure_tests.rs"]
mod start_failure_tests;
